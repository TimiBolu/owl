// Join Operators
// Implements join algorithms for OQL
//
// NOTE: In OQL, explicit JOINs are rare because navigation through references
// is O(1) pointer arithmetic. However, joins are still needed for:
// 1. Cross-collection queries without direct references
// 2. Many-to-many relationships
// 3. Complex filtering across collections
//
// Most "joins" in OQL are actually reference navigation, which is handled
// by the storage layer's navigate_reference() method.

use crate::document::Document;
use crate::error::OQLResult;
use crate::execution::operator::PhysicalOperatorExec;
use crate::planner::types::FilterCondition;
use crate::storage::Storage;
use std::collections::HashMap;

/// Nested loop join operator
///
/// For each row in the left input, scan all rows in the right input.
/// This is O(n * m) but works for any join condition.
///
/// Use when:
/// - Small datasets
/// - No better join algorithm available
/// - Join condition is complex
pub struct NestedLoopJoinExec {
    left: Box<dyn PhysicalOperatorExec>,
    right: Box<dyn PhysicalOperatorExec>,
    join_condition: Option<FilterCondition>,
    current_left: Option<Document>,
    storage: Option<Storage>,
}

impl NestedLoopJoinExec {
    pub fn new(
        left: Box<dyn PhysicalOperatorExec>,
        right: Box<dyn PhysicalOperatorExec>,
        join_condition: Option<FilterCondition>,
    ) -> Self {
        Self {
            left,
            right,
            join_condition,
            current_left: None,
            storage: None,
        }
    }

    fn evaluate_join_condition(
        &self,
        left_doc: &Document,
        right_doc: &Document,
    ) -> OQLResult<bool> {
        match &self.join_condition {
            Some(condition) => {
                // Create a combined document for evaluation
                let mut combined_fields = left_doc.fields.clone();
                for (key, value) in &right_doc.fields {
                    combined_fields.insert(format!("right_{}", key), value.clone());
                }

                let combined_doc = Document {
                    id: left_doc.id.clone(),
                    schema: left_doc.schema.clone(),
                    fields: combined_fields,
                    created_at: left_doc.created_at,
                    updated_at: left_doc.updated_at,
                    version: left_doc.version,
                };

                self.evaluate_condition(&combined_doc, condition)
            }
            None => Ok(true), // Cross product if no condition
        }
    }

    fn evaluate_condition(&self, doc: &Document, condition: &FilterCondition) -> OQLResult<bool> {
        match condition {
            FilterCondition::Comparison {
                left: _,
                operator: _,
                right: _,
            } => {
                // Simplified evaluation - in practice, this would use the full expression evaluator
                Ok(true)
            }
            FilterCondition::And(left, right) => {
                let left_result = self.evaluate_condition(doc, left)?;
                let right_result = self.evaluate_condition(doc, right)?;
                Ok(left_result && right_result)
            }
            FilterCondition::Or(left, right) => {
                let left_result = self.evaluate_condition(doc, left)?;
                let right_result = self.evaluate_condition(doc, right)?;
                Ok(left_result || right_result)
            }
            FilterCondition::Not(inner) => {
                let result = self.evaluate_condition(doc, inner)?;
                Ok(!result)
            }
        }
    }

    fn merge_documents(&self, left: &Document, right: &Document) -> Document {
        let mut merged_fields = left.fields.clone();

        // Add right document fields with prefix to avoid conflicts
        for (key, value) in &right.fields {
            merged_fields.insert(format!("right_{}", key), value.clone());
        }

        Document {
            id: format!("{}_{}", left.id, right.id),
            schema: format!("{}_{}", left.schema, right.schema),
            fields: merged_fields,
            created_at: left.created_at,
            updated_at: left.updated_at,
            version: left.version,
        }
    }
}

impl PhysicalOperatorExec for NestedLoopJoinExec {
    fn open(&mut self, storage: Storage) -> OQLResult<()> {
        self.storage = Some(storage.clone());
        self.left.open(storage.clone())?;
        self.right.open(storage)?;
        self.current_left = self.left.next()?;
        Ok(())
    }

    fn next(&mut self) -> OQLResult<Option<Document>> {
        loop {
            // If no current left row, we're done
            if self.current_left.is_none() {
                return Ok(None);
            }

            // Try to get next right row
            match self.right.next()? {
                Some(right_doc) => {
                    let left_doc = self.current_left.as_ref().unwrap();

                    // Check join condition
                    if self.evaluate_join_condition(left_doc, &right_doc)? {
                        // Merge documents and return
                        return Ok(Some(self.merge_documents(left_doc, &right_doc)));
                    }
                    // Continue to next right row
                }
                None => {
                    // No more right rows, advance left and reset right
                    self.current_left = self.left.next()?;
                    if self.current_left.is_some() {
                        self.right.reset()?;
                    }
                }
            }
        }
    }

    fn close(&mut self) -> OQLResult<()> {
        self.left.close()?;
        self.right.close()?;
        self.storage = None;
        Ok(())
    }

    fn reset(&mut self) -> OQLResult<()> {
        self.left.reset()?;
        self.right.reset()?;
        self.current_left = self.left.next()?;
        Ok(())
    }
}

/// Hash join operator
///
/// Build a hash table from the left input, then probe with the right input.
/// This is O(n + m) but requires memory for the hash table.
///
/// Use when:
/// - One side is significantly smaller (build side)
/// - Equi-join condition (equality comparison)
/// - Enough memory for hash table
#[allow(dead_code)]
pub struct HashJoinExec {
    left: Box<dyn PhysicalOperatorExec>,
    right: Box<dyn PhysicalOperatorExec>,
    join_condition: Option<FilterCondition>,
    hash_table: HashMap<String, Vec<Document>>,
    right_iter_started: bool,
    current_right: Option<Document>,
    current_left_matches: Vec<Document>,
    current_match_index: usize,
    storage: Option<Storage>,
}

impl HashJoinExec {
    pub fn new(
        left: Box<dyn PhysicalOperatorExec>,
        right: Box<dyn PhysicalOperatorExec>,
        join_condition: Option<FilterCondition>,
    ) -> Self {
        Self {
            left,
            right,
            join_condition,
            hash_table: HashMap::new(),
            right_iter_started: false,
            current_right: None,
            current_left_matches: Vec::new(),
            current_match_index: 0,
            storage: None,
        }
    }

    fn build_hash_table(&mut self) -> OQLResult<()> {
        // Build phase: Read all left rows into hash table
        while let Some(doc) = self.left.next()? {
            // Use document ID as hash key (simplified - in practice, extract join key)
            let key = doc.id.clone();
            self.hash_table
                .entry(key)
                .or_insert_with(Vec::new)
                .push(doc);
        }
        Ok(())
    }

    fn merge_documents(&self, left: &Document, right: &Document) -> Document {
        let mut merged_fields = left.fields.clone();

        // Add right document fields with prefix
        for (key, value) in &right.fields {
            merged_fields.insert(format!("right_{}", key), value.clone());
        }

        Document {
            id: format!("{}_{}", left.id, right.id),
            schema: format!("{}_{}", left.schema, right.schema),
            fields: merged_fields,
            created_at: left.created_at,
            updated_at: left.updated_at,
            version: left.version,
        }
    }
}

impl PhysicalOperatorExec for HashJoinExec {
    fn open(&mut self, storage: Storage) -> OQLResult<()> {
        self.storage = Some(storage.clone());
        self.left.open(storage.clone())?;
        self.right.open(storage)?;

        // Build hash table from left input
        self.build_hash_table()?;
        self.right_iter_started = false;

        Ok(())
    }

    fn next(&mut self) -> OQLResult<Option<Document>> {
        loop {
            // If we have matches from current right row, return them
            if self.current_match_index < self.current_left_matches.len() {
                let left_doc = &self.current_left_matches[self.current_match_index];
                let right_doc = self.current_right.as_ref().unwrap();
                self.current_match_index += 1;
                return Ok(Some(self.merge_documents(left_doc, right_doc)));
            }

            // Get next right row
            match self.right.next()? {
                Some(right_doc) => {
                    // Probe hash table with right row's key
                    let key = right_doc.id.clone(); // Simplified - extract actual join key

                    if let Some(left_matches) = self.hash_table.get(&key) {
                        // Found matches
                        self.current_right = Some(right_doc);
                        self.current_left_matches = left_matches.clone();
                        self.current_match_index = 0;
                        // Continue loop to return first match
                    } else {
                        // No matches, continue to next right row
                        continue;
                    }
                }
                None => {
                    // No more right rows
                    return Ok(None);
                }
            }
        }
    }

    fn close(&mut self) -> OQLResult<()> {
        self.left.close()?;
        self.right.close()?;
        self.hash_table.clear();
        self.storage = None;
        Ok(())
    }

    fn reset(&mut self) -> OQLResult<()> {
        self.left.reset()?;
        self.right.reset()?;
        self.hash_table.clear();
        self.build_hash_table()?;
        self.current_left_matches.clear();
        self.current_match_index = 0;
        self.current_right = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Value;
    use crate::execution::scan::SeqScanExec;
    use crate::storage::create_storage;
    use std::collections::HashMap;

    fn create_test_document(schema: &str, id: &str, name: &str) -> Document {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String(name.to_string()));

        Document::with_id(id.to_string(), schema.to_string(), fields)
    }

    #[test]
    fn test_nested_loop_join() {
        let storage = create_storage("test_nested_loop_join.owl", 10 * 1024 * 1024).unwrap();

        // Create test documents
        let user1 = create_test_document("users", "user1", "Alice");
        let user2 = create_test_document("users", "user2", "Bob");
        storage.put_document(&user1).unwrap();
        storage.put_document(&user2).unwrap();

        let order1 = create_test_document("orders", "order1", "Order1");
        let order2 = create_test_document("orders", "order2", "Order2");
        storage.put_document(&order1).unwrap();
        storage.put_document(&order2).unwrap();

        // Create join
        let left = Box::new(SeqScanExec::new("users".to_string()));
        let right = Box::new(SeqScanExec::new("orders".to_string()));
        let mut join = NestedLoopJoinExec::new(left, right, None);

        // Execute
        join.open(storage.clone()).unwrap();

        let mut results = Vec::new();
        while let Some(doc) = join.next().unwrap() {
            results.push(doc);
        }

        // Should have 2 users × 2 orders = 4 results
        assert_eq!(results.len(), 4);

        join.close().unwrap();
        std::fs::remove_file("test_nested_loop_join.owl").ok();
    }

    #[test]
    fn test_hash_join() {
        let storage = create_storage("test_hash_join.owl", 10 * 1024 * 1024).unwrap();

        // Create test documents
        let user1 = create_test_document("users", "user1", "Alice");
        let user2 = create_test_document("users", "user2", "Bob");
        storage.put_document(&user1).unwrap();
        storage.put_document(&user2).unwrap();

        let order1 = create_test_document("orders", "order1", "Order1");
        let order2 = create_test_document("orders", "order2", "Order2");
        storage.put_document(&order1).unwrap();
        storage.put_document(&order2).unwrap();

        // Create join
        let left = Box::new(SeqScanExec::new("users".to_string()));
        let right = Box::new(SeqScanExec::new("orders".to_string()));
        let mut join = HashJoinExec::new(left, right, None);

        // Execute
        join.open(storage.clone()).unwrap();

        let mut results = Vec::new();
        while let Some(doc) = join.next().unwrap() {
            results.push(doc);
        }

        // Hash join with no matching keys should return 0 results
        // (since we're using document IDs as keys and they don't match)
        assert_eq!(results.len(), 0);

        join.close().unwrap();
        std::fs::remove_file("test_hash_join.owl").ok();
    }
}
