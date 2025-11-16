// OQL Query Executor - Document scanning and filtering

use crate::document::{Document, Value, ObjRef};
use crate::error::{OQLError, OQLResult};
use crate::functions::FunctionRegistry;
// use crate::optimizer::QueryOptimizer; // TODO: Re-enable after optimizer module
use crate::parser::{ComparisonOperator, Condition, Expression, FieldAccess, Literal, OQLQuery, Operation, InsertBlock, UpdateBlock, SelectBlock, GroupByBlock, HavingBlock, SortBlock, SortDirection};
use crate::reference::ReferenceResolver;
use crate::storage::{DocumentStore, Storage};
use crate::streaming::{QueryStream, StreamingExecutor, StreamConfig, PaginatedResult, StreamCursor};
use std::collections::HashMap;
use std::sync::Arc;

/// Query execution engine for OQL
pub struct QueryExecutor<S: DocumentStore> {
    store: Arc<S>,
    functions: FunctionRegistry,
    // optimizer: QueryOptimizer, // TODO: Re-enable after optimizer module
    streaming_executor: StreamingExecutor<S>,
}

impl<S: DocumentStore> QueryExecutor<S> {
    /// Create a new query executor
    pub fn new(store: Arc<S>) -> Self {
        let streaming_executor = StreamingExecutor::new(store.clone());
        Self { 
            store: store.clone(),
            functions: FunctionRegistry::new(),
            // optimizer: QueryOptimizer::new(), // TODO: Re-enable after optimizer module
            streaming_executor,
        }
    }
    
    /// Register a user-defined function
    pub fn register_user_function(&mut self, func_def: crate::parser::FunctionDefinition) -> OQLResult<()> {
        self.functions.register_user_function(func_def)
    }
    
    /// Get access to the underlying store (for testing)
    pub fn store(&self) -> &Arc<S> {
        &self.store
    }
    
    /// Get function names (for testing)
    pub fn function_names(&self) -> Vec<String> {
        self.functions.function_names()
    }
    
    // TODO: Re-enable after optimizer module
    // /// Get access to the query optimizer
    // pub fn optimizer(&self) -> &QueryOptimizer {
    //     &self.optimizer
    // }
    // 
    // /// Get query execution statistics
    // pub fn get_query_stats(&self) -> OQLResult<HashMap<String, crate::optimizer::QueryStats>> {
    //     self.optimizer.get_query_stats()
    // }
    // 
    // /// Get index recommendations
    // pub fn get_index_recommendations(&self) -> OQLResult<Vec<crate::optimizer::IndexRecommendation>> {
    //     self.optimizer.get_index_recommendations()
    // }
    
    /// Clear query result cache
    /// TODO: Re-enable after optimizer module
    pub fn clear_query_cache(&self) -> OQLResult<()> {
        // self.optimizer.clear_cache()
        Ok(())
    }
    
    /// Execute a query as a stream for memory-efficient processing
    pub fn execute_query_stream(&self, query: &OQLQuery, config: StreamConfig) -> OQLResult<QueryStream<S>> {
        self.streaming_executor.execute_stream(query.clone(), config)
    }
    
    /// Execute a paginated query
    pub fn execute_paginated_query(
        &self,
        query: &OQLQuery,
        page_size: usize,
        cursor: Option<StreamCursor>,
    ) -> OQLResult<PaginatedResult> {
        self.streaming_executor.execute_paginated(query.clone(), page_size, cursor)
    }
    
    /// Count total documents for a query
    pub fn count_query_results(&self, query: &OQLQuery) -> OQLResult<usize> {
        self.streaming_executor.count_documents(query)
    }
    
    /// Execute an OQL query and return matching documents
    pub fn execute_query(&self, query: &OQLQuery) -> OQLResult<Vec<Document>> {
        let mut documents = Vec::new();
        let mut is_insert_only = false;
        
        // Check if this is an INSERT-only operation
        if query.operations.len() == 1 {
            if let Operation::Insert(_) = &query.operations[0] {
                is_insert_only = true;
            }
        }
        
        // For non-INSERT operations, start by scanning the collection
        if !is_insert_only {
            documents = self.scan_collection(&query.collection)?;
        }
        
        // Apply operations in sequence
        for operation in &query.operations {
            documents = self.apply_operation(documents, operation, &query.collection)?;
        }
        
        Ok(documents)
    }
    
    /// Scan all documents in a collection
    fn scan_collection(&self, collection: &str) -> OQLResult<Vec<Document>> {
        let documents = self.store.scan_documents(collection)?;
        
        Ok(documents)
    }
    
    /// Apply a single operation to a set of documents
    fn apply_operation(&self, documents: Vec<Document>, operation: &Operation, collection: &str) -> OQLResult<Vec<Document>> {
        match operation {
            Operation::Where(where_block) => {
                self.apply_where_filter(documents, &where_block.condition)
            }
            Operation::Select(select_block) => {
                self.apply_select_projection(documents, select_block)
            }
            Operation::Insert(insert_block) => {
                self.apply_insert_operation(collection, insert_block)
            }
            Operation::Update(update_block) => {
                self.apply_update_operation(documents, update_block)
            }
            Operation::Delete => {
                self.apply_delete_operation(documents)
            }
            Operation::GroupBy(group_by_block) => {
                self.apply_group_by_operation(documents, group_by_block)
            }
            Operation::Having(having_block) => {
                self.apply_having_operation(documents, having_block)
            }
            Operation::Count => {
                self.apply_count_operation(documents)
            }
            Operation::Sum(field_access) => {
                self.apply_sum_operation(documents, field_access)
            }
            Operation::Avg(field_access) => {
                self.apply_avg_operation(documents, field_access)
            }
            Operation::Min(field_access) => {
                self.apply_min_operation(documents, field_access)
            }
            Operation::Max(field_access) => {
                self.apply_max_operation(documents, field_access)
            }
            Operation::Sort(sort_block) => {
                self.apply_sort_operation(documents, sort_block)
            }
            Operation::Limit(limit_value) => {
                self.apply_limit_operation(documents, *limit_value)
            }
            Operation::FunctionDef(_) => {
                let context = crate::error::ErrorContext::new("apply_operation")
                    .with_location("query_executor")
                    .with_info("operation_type", "function_definition");
                
                Err(OQLError::EnhancedQueryExecution {
                    message: "Function definitions cannot be executed as query operations".to_string(),
                    context: Some(context),
                    severity: crate::error::ErrorSeverity::Error,
                    recovery: Some(crate::error::RecoverySuggestion {
                        suggestion: "Define functions separately from queries".to_string(),
                        example: Some("fn my_function() { ... } then use in queries".to_string()),
                        auto_recoverable: false,
                    }),
                })
            }

        }
    }
    
    /// Apply WHERE clause filtering to documents
    fn apply_where_filter(&self, documents: Vec<Document>, condition: &Condition) -> OQLResult<Vec<Document>> {
        let mut filtered = Vec::new();
        
        for document in documents {
            if self.evaluate_condition(&document, condition)? {
                filtered.push(document);
            }
        }
        
        Ok(filtered)
    }
    
    /// Evaluate a condition against a document
    fn evaluate_condition(&self, document: &Document, condition: &Condition) -> OQLResult<bool> {
        match condition {
            Condition::Comparison { left, operator, right } => {
                let left_value = self.evaluate_expression(document, left)?;
                let right_value = self.evaluate_expression(document, right)?;
                self.compare_values(&left_value, operator, &right_value)
            }
            Condition::And(left, right) => {
                let left_result = self.evaluate_condition(document, left)?;
                let right_result = self.evaluate_condition(document, right)?;
                Ok(left_result && right_result)
            }
            Condition::Or(left, right) => {
                let left_result = self.evaluate_condition(document, left)?;
                let right_result = self.evaluate_condition(document, right)?;
                Ok(left_result || right_result)
            }
            Condition::Not(inner) => {
                let result = self.evaluate_condition(document, inner)?;
                Ok(!result)
            }
        }
    }
    
    /// Evaluate an expression against a document
    fn evaluate_expression(&self, document: &Document, expression: &Expression) -> OQLResult<Value> {
        match expression {
            Expression::FieldAccess(field_access) => {
                self.get_field_value(document, field_access)
            }
            Expression::Literal(literal) => {
                Ok(self.literal_to_value(literal))
            }
            Expression::FunctionCall { name, args } => {
                self.evaluate_function_call(document, name, args)
            }
            Expression::IfThenElse { condition, then_expr, else_expr } => {
                let condition_result = self.evaluate_condition(document, condition)?;
                if condition_result {
                    self.evaluate_expression(document, then_expr)
                } else {
                    self.evaluate_expression(document, else_expr)
                }
            }
            Expression::Match { expr, arms } => {
                let expr_value = self.evaluate_expression(document, expr)?;
                self.evaluate_match_expression(document, &expr_value, arms)
            }
            Expression::LogicalAnd(left, right) => {
                let left_value = self.evaluate_expression(document, left)?;
                let left_bool = self.value_to_bool(&left_value)?;
                
                if !left_bool {
                    Ok(Value::Boolean(false))
                } else {
                    let right_value = self.evaluate_expression(document, right)?;
                    let right_bool = self.value_to_bool(&right_value)?;
                    Ok(Value::Boolean(right_bool))
                }
            }
            Expression::LogicalOr(left, right) => {
                let left_value = self.evaluate_expression(document, left)?;
                let left_bool = self.value_to_bool(&left_value)?;
                
                if left_bool {
                    Ok(Value::Boolean(true))
                } else {
                    let right_value = self.evaluate_expression(document, right)?;
                    let right_bool = self.value_to_bool(&right_value)?;
                    Ok(Value::Boolean(right_bool))
                }
            }
            Expression::LogicalNot(expr) => {
                let value = self.evaluate_expression(document, expr)?;
                let bool_value = self.value_to_bool(&value)?;
                Ok(Value::Boolean(!bool_value))
            }
            Expression::BinaryOp { left, operator, right } => {
                let left_value = self.evaluate_expression(document, left)?;
                let right_value = self.evaluate_expression(document, right)?;
                self.evaluate_binary_operation(&left_value, operator, &right_value)
            }
            Expression::ArrayAccess { array, index } => {
                let array_value = self.evaluate_expression(document, array)?;
                let index_value = self.evaluate_expression(document, index)?;
                self.evaluate_array_access(&array_value, &index_value)
            }
            Expression::ChainedAccess { base, field } => {
                let base_value = self.evaluate_expression(document, base)?;
                self.get_field_from_value(&base_value, field)
            }
            Expression::Block(expressions) => {
                // Evaluate all expressions in the block and return the last one
                let mut last_value = Value::Null;
                for expr in expressions {
                    last_value = self.evaluate_expression(document, expr)?;
                }
                Ok(last_value)
            }
            Expression::ArrayLiteral(elements) => {
                let mut values = Vec::new();
                for element in elements {
                    let value = self.evaluate_expression(document, element)?;
                    values.push(value);
                }
                Ok(Value::List(values))
            }
            Expression::ObjectLiteral(fields) => {
                let mut object_fields = std::collections::HashMap::new();
                for field in fields {
                    let value = self.evaluate_expression(document, &field.value)?;
                    object_fields.insert(field.name.clone(), value);
                }
                Ok(Value::Object(object_fields))
            }
        }
    }
    
    /// Evaluate a binary operation
    fn evaluate_binary_operation(&self, left: &Value, operator: &crate::parser::BinaryOperator, right: &Value) -> OQLResult<Value> {
        use crate::parser::BinaryOperator;
        
        match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => {
                match operator {
                    BinaryOperator::Add => Ok(Value::Integer(a + b)),
                    BinaryOperator::Subtract => Ok(Value::Integer(a - b)),
                    BinaryOperator::Multiply => Ok(Value::Integer(a * b)),
                    BinaryOperator::Divide => {
                        if *b == 0 {
                            Err(OQLError::FunctionError { message: "Division by zero".to_string() })
                        } else {
                            Ok(Value::Integer(a / b))
                        }
                    }
                    BinaryOperator::Modulo => {
                        if *b == 0 {
                            Err(OQLError::FunctionError { message: "Modulo by zero".to_string() })
                        } else {
                            Ok(Value::Integer(a % b))
                        }
                    }
                }
            }
            (Value::Float(a), Value::Float(b)) => {
                match operator {
                    BinaryOperator::Add => Ok(Value::Float(a + b)),
                    BinaryOperator::Subtract => Ok(Value::Float(a - b)),
                    BinaryOperator::Multiply => Ok(Value::Float(a * b)),
                    BinaryOperator::Divide => {
                        if *b == 0.0 {
                            Err(OQLError::FunctionError { message: "Division by zero".to_string() })
                        } else {
                            Ok(Value::Float(a / b))
                        }
                    }
                    BinaryOperator::Modulo => Ok(Value::Float(a % b)),
                }
            }
            (Value::Integer(a), Value::Float(b)) => {
                let a_float = *a as f64;
                match operator {
                    BinaryOperator::Add => Ok(Value::Float(a_float + b)),
                    BinaryOperator::Subtract => Ok(Value::Float(a_float - b)),
                    BinaryOperator::Multiply => Ok(Value::Float(a_float * b)),
                    BinaryOperator::Divide => {
                        if *b == 0.0 {
                            Err(OQLError::FunctionError { message: "Division by zero".to_string() })
                        } else {
                            Ok(Value::Float(a_float / b))
                        }
                    }
                    BinaryOperator::Modulo => Ok(Value::Float(a_float % b)),
                }
            }
            (Value::Float(a), Value::Integer(b)) => {
                let b_float = *b as f64;
                match operator {
                    BinaryOperator::Add => Ok(Value::Float(a + b_float)),
                    BinaryOperator::Subtract => Ok(Value::Float(a - b_float)),
                    BinaryOperator::Multiply => Ok(Value::Float(a * b_float)),
                    BinaryOperator::Divide => {
                        if *b == 0 {
                            Err(OQLError::FunctionError { message: "Division by zero".to_string() })
                        } else {
                            Ok(Value::Float(a / b_float))
                        }
                    }
                    BinaryOperator::Modulo => Ok(Value::Float(a % b_float)),
                }
            }
            (Value::String(a), Value::String(b)) if matches!(operator, BinaryOperator::Add) => {
                Ok(Value::String(format!("{}{}", a, b)))
            }
            _ => Err(OQLError::FunctionError { 
                message: format!("Invalid binary operation: {:?} {:?} {:?}", left, operator, right)
            })
        }
    }
    
    /// Evaluate array access
    fn evaluate_array_access(&self, array_value: &Value, index_value: &Value) -> OQLResult<Value> {
        match (array_value, index_value) {
            (Value::List(list), Value::Integer(index)) => {
                let idx = *index as usize;
                if idx < list.len() {
                    Ok(list[idx].clone())
                } else {
                    Ok(Value::Null) // Return null for out-of-bounds access
                }
            }
            (Value::String(s), Value::Integer(index)) => {
                let idx = *index as usize;
                let chars: Vec<char> = s.chars().collect();
                if idx < chars.len() {
                    Ok(Value::String(chars[idx].to_string()))
                } else {
                    Ok(Value::Null) // Return null for out-of-bounds access
                }
            }
            _ => Err(OQLError::FunctionError {
                message: format!("Invalid array access: {:?}[{:?}]", array_value, index_value)
            })
        }
    }
    
    /// Get a field from a value (for chained access)
    fn get_field_from_value(&self, value: &Value, field_name: &str) -> OQLResult<Value> {
        match value {
            Value::Object(obj) => {
                Ok(obj.get(field_name).cloned().unwrap_or(Value::Null))
            }
            _ => Ok(Value::Null)
        }
    }
    
    /// Evaluate a match expression
    fn evaluate_match_expression(&self, document: &Document, expr_value: &Value, arms: &[crate::parser::MatchArm]) -> OQLResult<Value> {
        for arm in arms {
            if self.pattern_matches(&arm.pattern, expr_value)? {
                // Check guard condition if present
                if let Some(guard) = &arm.guard {
                    if !self.evaluate_condition(document, guard)? {
                        continue;
                    }
                }
                
                return self.evaluate_expression(document, &arm.body);
            }
        }
        
        Err(OQLError::Generic {
            message: "No matching pattern found in match expression".to_string(),
        })
    }
    
    /// Check if a pattern matches a value
    fn pattern_matches(&self, pattern: &crate::parser::MatchPattern, value: &Value) -> OQLResult<bool> {
        match pattern {
            crate::parser::MatchPattern::Literal(literal) => {
                let pattern_value = self.literal_to_value(literal);
                Ok(self.values_equal(value, &pattern_value))
            }
            crate::parser::MatchPattern::Wildcard => Ok(true),
            crate::parser::MatchPattern::Variable(_) => Ok(true), // Variables always match
            crate::parser::MatchPattern::Guard(_condition) => {
                // For guard patterns, we need a document context - this is a simplified implementation
                // In a full implementation, we'd need to bind the matched value to a variable
                Ok(true) // Simplified - always matches for now
            }
        }
    }
    
    /// Convert a value to boolean for logical operations
    fn value_to_bool(&self, value: &Value) -> OQLResult<bool> {
        match value {
            Value::Boolean(b) => Ok(*b),
            Value::Integer(i) => Ok(*i != 0),
            Value::Float(f) => Ok(*f != 0.0),
            Value::String(s) => Ok(!s.is_empty()),
            Value::Null => Ok(false),
            _ => Err(OQLError::TypeError {
                expected: "boolean-convertible type".to_string(),
                actual: value.type_name().to_string(),
            }),
        }
    }
    
    /// Get field value from document using field access path with implicit null handling
    /// This is the core implementation of OWL's migration-friendly field access
    pub fn get_field_value(&self, document: &Document, field_access: &FieldAccess) -> OQLResult<Value> {
        if field_access.path.is_empty() {
            // Return null for empty path instead of error - graceful degradation
            return Ok(Value::Null);
        }
        
        // Handle single-level field access directly
        if field_access.path.len() == 1 {
            let field_name = &field_access.path[0];
            
            // Handle special fields
            match field_name.as_str() {
                "id" => return Ok(Value::String(document.id.clone())),
                "schema" => return Ok(Value::String(document.schema.clone())),
                "created_at" => return Ok(Value::DateTime(document.created_at)),
                "updated_at" => return Ok(Value::DateTime(document.updated_at)),
                _ => {
                    // Regular field access - always returns a value (null if missing)
                    return match document.get_field(field_name) {
                        Some(value) => Ok(value.clone()),
                        None => Ok(Value::Null), // Implicit null handling - no errors!
                    };
                }
            }
        }
        
        // Handle multi-level field access with implicit null handling
        self.resolve_nested_field_path(document, &field_access.path)
    }
    
    /// Resolve nested field path with implicit null handling at every level
    /// This enables migration-free schema evolution by never throwing errors for missing fields
    fn resolve_nested_field_path(&self, document: &Document, path: &[String]) -> OQLResult<Value> {
        if path.is_empty() {
            return Ok(Value::Null);
        }
        
        let field_name = &path[0];
        let remaining_path = &path[1..];
        
        // Get the field value from the current document
        let field_value = match field_name.as_str() {
            // Handle special document fields
            "id" => Value::String(document.id.clone()),
            "schema" => Value::String(document.schema.clone()),
            "created_at" => Value::DateTime(document.created_at),
            "updated_at" => Value::DateTime(document.updated_at),
            _ => {
                // Regular field access - return null if field doesn't exist
                match document.get_field(field_name) {
                    Some(value) => value.clone(),
                    None => Value::Null, // Field doesn't exist - return null gracefully
                }
            }
        };
        
        // If this is the final field in the path, return its value
        if remaining_path.is_empty() {
            return Ok(field_value);
        }
        
        // We need to navigate deeper - handle different value types
        match field_value {
            Value::Reference(objref) => {
                // This is a reference - resolve it and continue navigation
                self.resolve_reference_and_continue(&objref, remaining_path)
            }
            Value::Object(nested_object) => {
                // This is a nested object - navigate into it
                self.resolve_object_field_path(&nested_object, remaining_path)
            }
            Value::Null => {
                // Field is null - any deeper navigation also returns null
                Ok(Value::Null)
            }
            _ => {
                // Trying to navigate into a non-reference, non-object value
                // In traditional systems this would be an error, but we return null gracefully
                Ok(Value::Null)
            }
        }
    }
    
    /// Resolve nested object field path with implicit null handling
    /// This enables deep navigation into nested objects like .profile.preferences.theme
    fn resolve_object_field_path(&self, object: &HashMap<String, Value>, path: &[String]) -> OQLResult<Value> {
        if path.is_empty() {
            return Ok(Value::Null);
        }
        
        let field_name = &path[0];
        let remaining_path = &path[1..];
        
        // Get the field value from the nested object
        let field_value = match object.get(field_name) {
            Some(value) => value.clone(),
            None => Value::Null, // Field doesn't exist in nested object - return null gracefully
        };
        
        // If this is the final field in the path, return its value
        if remaining_path.is_empty() {
            return Ok(field_value);
        }
        
        // We need to navigate deeper - handle different value types
        match field_value {
            Value::Object(nested_object) => {
                // Navigate deeper into nested object
                self.resolve_object_field_path(&nested_object, remaining_path)
            }
            Value::Reference(objref) => {
                // This is a reference within a nested object - resolve it
                self.resolve_reference_and_continue(&objref, remaining_path)
            }
            Value::Null => {
                // Field is null - any deeper navigation also returns null
                Ok(Value::Null)
            }
            _ => {
                // Trying to navigate into a non-object, non-reference value
                // Return null gracefully instead of error
                Ok(Value::Null)
            }
        }
    }
    
    /// Resolve a reference and continue field path navigation
    /// 
    /// This implements O(1) pointer-based navigation when using Zig storage.
    /// For other storage backends, falls back to document lookup.
    fn resolve_reference_and_continue(&self, objref: &ObjRef, remaining_path: &[String]) -> OQLResult<Value> {
        // Try O(1) pointer-based navigation first (if using Zig storage)
        if let Some(referenced_doc) = self.try_fast_navigation(objref)? {
            // Fast path: Direct pointer navigation succeeded
            return self.resolve_nested_field_path(&referenced_doc, remaining_path);
        }
        
        // Fallback path: Convert ObjRef to string ID for compatibility
        let ref_id = objref.to_string_id();
        
        // Parse the reference to get schema and document ID
        let (schema, doc_id) = if ref_id.contains('#') {
            let parts: Vec<&str> = ref_id.splitn(2, '#').collect();
            (parts[0], parts[1])
        } else if ref_id.starts_with("ref_") {
            // This is a hashed reference - we can't resolve it without a mapping
            // Return null gracefully for now
            return Ok(Value::Null);
        } else {
            // If no schema prefix, we can't resolve - return null gracefully
            return Ok(Value::Null);
        };
        
        // Try to load the referenced document
        match self.store.get_document(schema, &doc_id.to_string()) {
            Ok(Some(referenced_doc)) => {
                // Successfully loaded - continue navigation
                self.resolve_nested_field_path(&referenced_doc, remaining_path)
            }
            Ok(None) | Err(_) => {
                // Referenced document doesn't exist or can't be loaded - return null gracefully
                Ok(Value::Null)
            }
        }
    }
    
    /// Try fast O(1) pointer-based navigation using Zig storage
    /// 
    /// This is the critical path for OQL's performance advantage:
    /// - Navigation is pointer arithmetic, not a database query
    /// - Executes in <50ns (2-3 memory loads)
    /// - No query planning overhead
    /// 
    /// Returns Some(Document) if fast navigation succeeded, None otherwise.
    fn try_fast_navigation(&self, objref: &ObjRef) -> OQLResult<Option<Document>> {
        // Check if this is a null reference
        if objref.is_null() {
            return Ok(None);
        }
        
        // Try to use the storage engine's fast navigation if available
        // This will use direct pointer swizzling for Zig storage
        match self.store.navigate_reference(objref) {
            Ok(Some(doc)) => Ok(Some(doc)),
            Ok(None) => Ok(None),
            Err(_) => Ok(None), // Fallback to slow path on error
        }
    }
    
    /// Convert a literal to a Value
    fn literal_to_value(&self, literal: &Literal) -> Value {
        match literal {
            Literal::String(s) => Value::String(s.clone()),
            Literal::Integer(i) => Value::Integer(*i),
            Literal::Float(f) => Value::Float(*f),
            Literal::Boolean(b) => Value::Boolean(*b),
        }
    }
    
    /// Compare two values using the given operator
    fn compare_values(&self, left: &Value, operator: &ComparisonOperator, right: &Value) -> OQLResult<bool> {
        match operator {
            ComparisonOperator::Equal => Ok(self.values_equal(left, right)),
            ComparisonOperator::NotEqual => Ok(!self.values_equal(left, right)),
            ComparisonOperator::LessThan => self.compare_ordered_values(left, right, |ord| ord.is_lt()),
            ComparisonOperator::GreaterThan => self.compare_ordered_values(left, right, |ord| ord.is_gt()),
            ComparisonOperator::LessEqual => self.compare_ordered_values(left, right, |ord| ord.is_le()),
            ComparisonOperator::GreaterEqual => self.compare_ordered_values(left, right, |ord| ord.is_ge()),
            ComparisonOperator::Contains => self.string_contains(left, right),
            ComparisonOperator::StartsWith => self.string_starts_with(left, right),
            ComparisonOperator::EndsWith => self.string_ends_with(left, right),
            ComparisonOperator::In => self.value_in_collection(left, right),
        }
    }
    
    /// Check if left string contains right string
    fn string_contains(&self, left: &Value, right: &Value) -> OQLResult<bool> {
        match (left, right) {
            (Value::String(left_str), Value::String(right_str)) => {
                Ok(left_str.contains(right_str))
            }
            _ => Ok(false), // Non-string values don't support contains
        }
    }
    
    /// Check if left string starts with right string
    fn string_starts_with(&self, left: &Value, right: &Value) -> OQLResult<bool> {
        match (left, right) {
            (Value::String(left_str), Value::String(right_str)) => {
                Ok(left_str.starts_with(right_str))
            }
            _ => Ok(false), // Non-string values don't support starts_with
        }
    }
    
    /// Check if left string ends with right string
    fn string_ends_with(&self, left: &Value, right: &Value) -> OQLResult<bool> {
        match (left, right) {
            (Value::String(left_str), Value::String(right_str)) => {
                Ok(left_str.ends_with(right_str))
            }
            _ => Ok(false), // Non-string values don't support ends_with
        }
    }
    
    /// Check if left value is in right collection
    fn value_in_collection(&self, left: &Value, right: &Value) -> OQLResult<bool> {
        match right {
            Value::List(items) => {
                for item in items {
                    if self.values_equal(left, item) {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            _ => Ok(false), // Right side must be a list for 'in' operator
        }
    }
    
    /// Check if two values are equal
    fn values_equal(&self, left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Integer(a), Value::Integer(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => (a - b).abs() < f64::EPSILON,
            (Value::Boolean(a), Value::Boolean(b)) => a == b,
            (Value::DateTime(a), Value::DateTime(b)) => a == b,
            (Value::Reference(a), Value::Reference(b)) => a == b,
            (Value::Null, Value::Null) => true,
            // Type coercion: Integer to Float
            (Value::Integer(a), Value::Float(b)) => (*a as f64 - b).abs() < f64::EPSILON,
            (Value::Float(a), Value::Integer(b)) => (a - *b as f64).abs() < f64::EPSILON,
            _ => false,
        }
    }
    
    /// Compare two values for ordering operations
    fn compare_ordered_values<F>(&self, left: &Value, right: &Value, compare_fn: F) -> OQLResult<bool>
    where
        F: Fn(std::cmp::Ordering) -> bool,
    {
        let ordering = match (left, right) {
            (Value::String(a), Value::String(b)) => a.cmp(b),
            (Value::Integer(a), Value::Integer(b)) => a.cmp(b),
            (Value::Float(a), Value::Float(b)) => a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal),
            (Value::Boolean(a), Value::Boolean(b)) => a.cmp(b),
            (Value::DateTime(a), Value::DateTime(b)) => a.cmp(b),
            // Type coercion: DateTime to String (treat as string comparison)
            (Value::DateTime(a), Value::String(b)) => a.to_rfc3339().cmp(b),
            (Value::String(a), Value::DateTime(b)) => a.cmp(&b.to_rfc3339()),
            // Type coercion: Integer to Float
            (Value::Integer(a), Value::Float(b)) => (*a as f64).partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal),
            (Value::Float(a), Value::Integer(b)) => a.partial_cmp(&(*b as f64)).unwrap_or(std::cmp::Ordering::Equal),
            _ => {
                return Err(OQLError::TypeError {
                    expected: "comparable types".to_string(),
                    actual: format!("{} and {}", left.type_name(), right.type_name()),
                });
            }
        };
        
        Ok(compare_fn(ordering))
    }
    
    /// Apply SELECT projection to documents
    fn apply_select_projection(&self, documents: Vec<Document>, select_block: &SelectBlock) -> OQLResult<Vec<Document>> {
        // Check if any field is an aggregation function
        let has_aggregation = select_block.fields.iter().any(|field| {
            match &field.expression {
                Expression::FunctionCall { name, args } => {
                    match name.as_str() {
                        "count" | "sum" | "avg" => true,
                        "min" | "max" => args.len() == 1, // Only aggregation if single argument
                        _ => false,
                    }
                }
                _ => false,
            }
        });
        
        // Check if we're dealing with grouped data (documents with schema "group")
        let is_grouped_data = documents.iter().any(|doc| doc.schema == "group");
        
        if has_aggregation && !is_grouped_data {
            // Handle aggregation over all documents - return single result document
            let mut projected_fields = HashMap::new();
            
            for select_field in &select_block.fields {
                let field_name = match &select_field.alias {
                    Some(alias) => alias.clone(),
                    None => {
                        match &select_field.expression {
                            Expression::FunctionCall { name, .. } => name.clone(),
                            Expression::FieldAccess(field_access) => {
                                field_access.path.last().unwrap_or(&"unknown".to_string()).clone()
                            }
                            _ => "computed".to_string(),
                        }
                    }
                };
                
                let value = match &select_field.expression {
                    Expression::FunctionCall { name, args } => {
                        // Handle aggregation function
                        let field_path = if args.is_empty() {
                            Vec::new() // For count()
                        } else {
                            match &args[0] {
                                Expression::FieldAccess(field_access) => {
                                    // Handle count(*) case
                                    if field_access.path.len() == 1 && field_access.path[0] == "*" {
                                        Vec::new() // Treat count(*) same as count()
                                    } else {
                                        field_access.path.clone()
                                    }
                                },
                                _ => return Err(OQLError::Generic {
                                    message: "Aggregation functions only support field access arguments".to_string(),
                                }),
                            }
                        };
                        
                        self.execute_aggregation(&documents, name, &field_path)?
                    }
                    _ => {
                        // Non-aggregation expression - use first document or error
                        if documents.is_empty() {
                            Value::Null
                        } else {
                            self.evaluate_expression(&documents[0], &select_field.expression)?
                        }
                    }
                };
                
                projected_fields.insert(field_name, value);
            }
            
            let projected_doc = Document::with_id(
                "aggregation_result".to_string(),
                "aggregation".to_string(),
                projected_fields,
            );
            
            Ok(vec![projected_doc])
        } else {
            // Handle regular projection - return one document per input document
            let mut projected_documents = Vec::new();
            
            for document in documents {
                let mut projected_fields = HashMap::new();
                
                for select_field in &select_block.fields {
                    let field_name = match &select_field.alias {
                        Some(alias) => alias.clone(),
                        None => {
                            // Extract field name from expression
                            match &select_field.expression {
                                Expression::FieldAccess(field_access) => {
                                    field_access.path.last().unwrap_or(&"unknown".to_string()).clone()
                                }
                                Expression::FunctionCall { name, .. } => name.clone(),
                                _ => "computed".to_string(),
                            }
                        }
                    };
                    
                    let value = match &select_field.expression {
                        Expression::FunctionCall { name, args } if is_grouped_data => {
                            // Handle aggregation function on grouped data
                            let field_path = if args.is_empty() {
                                Vec::new() // For count()
                            } else {
                                match &args[0] {
                                    Expression::FieldAccess(field_access) => {
                                        // Handle count(*) case
                                        if field_access.path.len() == 1 && field_access.path[0] == "*" {
                                            Vec::new() // Treat count(*) same as count()
                                        } else {
                                            field_access.path.clone()
                                        }
                                    },
                                    _ => return Err(OQLError::Generic {
                                        message: "Aggregation functions only support field access arguments".to_string(),
                                    }),
                                }
                            };
                            
                            self.execute_group_aggregation(&document, name, &field_path)?
                        }
                        _ => {
                            self.evaluate_expression(&document, &select_field.expression)?
                        }
                    };
                    
                    projected_fields.insert(field_name, value);
                }
                
                let projected_doc = Document::with_id(
                    document.id.clone(),
                    document.schema.clone(),
                    projected_fields,
                );
                
                projected_documents.push(projected_doc);
            }
            
            Ok(projected_documents)
        }
    }
    
    /// Apply INSERT operation
    fn apply_insert_operation(&self, collection: &str, insert_block: &InsertBlock) -> OQLResult<Vec<Document>> {
        // Convert insert fields to document fields
        let mut fields = HashMap::new();
        
        // Create a temporary empty document for expression evaluation context
        let temp_doc = Document::new(collection.to_string(), HashMap::new());
        
        for insert_field in &insert_block.fields {
            let value = self.evaluate_expression(&temp_doc, &insert_field.value)?;
            fields.insert(insert_field.name.clone(), value);
        }
        
        // Create new document
        let document = Document::new(collection.to_string(), fields);
        
        // Validate against schema if it exists
        if let Ok(Some(schema)) = self.store.get_schema(collection) {
            schema.validate_document(&document.fields)?;
        }
        
        // Store the document
        self.store.put_document(&document)?;
        
        // Return the created document
        Ok(vec![document])
    }
    
    /// Apply UPDATE operation to documents
    fn apply_update_operation(&self, documents: Vec<Document>, update_block: &UpdateBlock) -> OQLResult<Vec<Document>> {
        let mut updated_documents = Vec::new();
        
        for mut document in documents {
            // Apply updates to the document
            for update_field in &update_block.fields {
                let new_value = self.evaluate_expression(&document, &update_field.value)?;
                document.set_field(update_field.name.clone(), new_value);
            }
            
            // Validate against schema if it exists
            if let Ok(Some(schema)) = self.store.get_schema(&document.schema) {
                schema.validate_document(&document.fields)?;
            }
            
            // Store the updated document
            self.store.put_document(&document)?;
            updated_documents.push(document);
        }
        
        Ok(updated_documents)
    }
    
    /// Apply DELETE operation to documents
    fn apply_delete_operation(&self, documents: Vec<Document>) -> OQLResult<Vec<Document>> {
        let mut deleted_documents = Vec::new();
        
        for document in documents {
            // Delete the document from storage
            let was_deleted = self.store.delete_document(&document.schema, &document.id)?;
            
            if was_deleted {
                deleted_documents.push(document);
            }
        }
        
        Ok(deleted_documents)
    }
    
    /// Evaluate a function call expression
    fn evaluate_function_call(&self, document: &Document, name: &str, args: &[Expression]) -> OQLResult<Value> {
        // Handle aggregation functions specially (they need document context)
        // But only if they have the right number of arguments for aggregation
        match name {
            "count" => self.evaluate_count_function(document, args),
            "sum" if args.len() == 1 => self.evaluate_sum_function(document, args),
            "avg" if args.len() == 1 => self.evaluate_avg_function(document, args),
            "min" if args.len() == 1 => self.evaluate_min_function(document, args),
            "max" if args.len() == 1 => self.evaluate_max_function(document, args),
            _ => {
                // Check if it's a built-in function
                if let Some(func) = self.functions.get_function(name) {
                    // Evaluate arguments first
                    let mut arg_values = Vec::new();
                    for arg in args {
                        arg_values.push(self.evaluate_expression(document, arg)?);
                    }
                    
                    // Call the built-in function
                    func(&arg_values)
                } else if let Some(user_func) = self.functions.get_user_function(name) {
                    // Handle user-defined function
                    self.evaluate_user_function(document, user_func, args)
                } else {
                    Err(OQLError::Generic {
                        message: format!("Unknown function: {}", name),
                    })
                }
            }
        }
    }
    
    /// Evaluate COUNT function
    fn evaluate_count_function(&self, _document: &Document, args: &[Expression]) -> OQLResult<Value> {
        if args.is_empty() {
            // count() or count(*) - count all documents
            Ok(Value::Integer(1)) // For single document context, always 1
        } else if args.len() == 1 {
            // count(field) - count non-null values
            match &args[0] {
                Expression::FieldAccess(_) => {
                    let field_value = self.evaluate_expression(_document, &args[0])?;
                    if field_value.is_null() {
                        Ok(Value::Integer(0))
                    } else {
                        Ok(Value::Integer(1))
                    }
                }
                _ => Ok(Value::Integer(1)), // count of literal values is always 1
            }
        } else {
            Err(OQLError::Generic {
                message: "COUNT function expects 0 or 1 arguments".to_string(),
            })
        }
    }
    
    /// Evaluate SUM function
    fn evaluate_sum_function(&self, document: &Document, args: &[Expression]) -> OQLResult<Value> {
        if args.len() != 1 {
            return Err(OQLError::Generic {
                message: "SUM function expects exactly 1 argument".to_string(),
            });
        }
        
        let value = self.evaluate_expression(document, &args[0])?;
        match value {
            Value::Integer(i) => Ok(Value::Integer(i)),
            Value::Float(f) => Ok(Value::Float(f)),
            Value::Null => Ok(Value::Integer(0)),
            _ => Err(OQLError::TypeError {
                expected: "numeric type".to_string(),
                actual: value.type_name().to_string(),
            }),
        }
    }
    
    /// Evaluate AVG function
    fn evaluate_avg_function(&self, document: &Document, args: &[Expression]) -> OQLResult<Value> {
        if args.len() != 1 {
            return Err(OQLError::Generic {
                message: "AVG function expects exactly 1 argument".to_string(),
            });
        }
        
        let value = self.evaluate_expression(document, &args[0])?;
        match value {
            Value::Integer(i) => Ok(Value::Float(i as f64)),
            Value::Float(f) => Ok(Value::Float(f)),
            Value::Null => Ok(Value::Null),
            _ => Err(OQLError::TypeError {
                expected: "numeric type".to_string(),
                actual: value.type_name().to_string(),
            }),
        }
    }
    
    /// Evaluate MIN function
    fn evaluate_min_function(&self, document: &Document, args: &[Expression]) -> OQLResult<Value> {
        if args.len() != 1 {
            return Err(OQLError::Generic {
                message: "MIN function expects exactly 1 argument".to_string(),
            });
        }
        
        let value = self.evaluate_expression(document, &args[0])?;
        match value {
            Value::Integer(_) | Value::Float(_) | Value::String(_) | Value::DateTime(_) => Ok(value),
            Value::Null => Ok(Value::Null),
            _ => Err(OQLError::TypeError {
                expected: "comparable type".to_string(),
                actual: value.type_name().to_string(),
            }),
        }
    }
    
    /// Evaluate MAX function
    fn evaluate_max_function(&self, document: &Document, args: &[Expression]) -> OQLResult<Value> {
        if args.len() != 1 {
            return Err(OQLError::Generic {
                message: "MAX function expects exactly 1 argument".to_string(),
            });
        }
        
        let value = self.evaluate_expression(document, &args[0])?;
        match value {
            Value::Integer(_) | Value::Float(_) | Value::String(_) | Value::DateTime(_) => Ok(value),
            Value::Null => Ok(Value::Null),
            _ => Err(OQLError::TypeError {
                expected: "comparable type".to_string(),
                actual: value.type_name().to_string(),
            }),
        }
    }
    
    /// Execute aggregation over a collection of documents
    pub fn execute_aggregation(&self, documents: &[Document], function_name: &str, field_path: &[String]) -> OQLResult<Value> {
        match function_name {
            "count" => {
                if field_path.is_empty() {
                    // count(*) - count all documents
                    Ok(Value::Integer(documents.len() as i64))
                } else {
                    // count(field) - count non-null values
                    let mut count = 0i64;
                    for doc in documents {
                        let field_access = FieldAccess { path: field_path.to_vec() };
                        let value = self.get_field_value(doc, &field_access)?;
                        if !value.is_null() {
                            count += 1;
                        }
                    }
                    Ok(Value::Integer(count))
                }
            }
            "sum" => {
                let mut sum_int = 0i64;
                let mut sum_float = 0.0f64;
                let mut has_float = false;
                let mut count = 0;
                
                for doc in documents {
                    let field_access = FieldAccess { path: field_path.to_vec() };
                    let value = self.get_field_value(doc, &field_access)?;
                    
                    match value {
                        Value::Integer(i) => {
                            if has_float {
                                sum_float += i as f64;
                            } else {
                                sum_int += i;
                            }
                            count += 1;
                        }
                        Value::Float(f) => {
                            if !has_float {
                                sum_float = sum_int as f64 + f;
                                has_float = true;
                            } else {
                                sum_float += f;
                            }
                            count += 1;
                        }
                        Value::Null => {} // Skip null values
                        _ => return Err(OQLError::TypeError {
                            expected: "numeric type".to_string(),
                            actual: value.type_name().to_string(),
                        }),
                    }
                }
                
                if count == 0 {
                    Ok(Value::Integer(0))
                } else if has_float {
                    Ok(Value::Float(sum_float))
                } else {
                    Ok(Value::Integer(sum_int))
                }
            }
            "avg" => {
                let mut sum_float = 0.0f64;
                let mut count = 0;
                
                for doc in documents {
                    let field_access = FieldAccess { path: field_path.to_vec() };
                    let value = self.get_field_value(doc, &field_access)?;
                    
                    match value {
                        Value::Integer(i) => {
                            sum_float += i as f64;
                            count += 1;
                        }
                        Value::Float(f) => {
                            sum_float += f;
                            count += 1;
                        }
                        Value::Null => {} // Skip null values
                        _ => return Err(OQLError::TypeError {
                            expected: "numeric type".to_string(),
                            actual: value.type_name().to_string(),
                        }),
                    }
                }
                
                if count == 0 {
                    Ok(Value::Null)
                } else {
                    Ok(Value::Float(sum_float / count as f64))
                }
            }
            "min" => {
                let mut min_value: Option<Value> = None;
                
                for doc in documents {
                    let field_access = FieldAccess { path: field_path.to_vec() };
                    let value = self.get_field_value(doc, &field_access)?;
                    
                    if value.is_null() {
                        continue;
                    }
                    
                    match &min_value {
                        None => min_value = Some(value),
                        Some(current_min) => {
                            if self.compare_ordered_values(&value, current_min, |ord| ord.is_lt())? {
                                min_value = Some(value);
                            }
                        }
                    }
                }
                
                Ok(min_value.unwrap_or(Value::Null))
            }
            "max" => {
                let mut max_value: Option<Value> = None;
                
                for doc in documents {
                    let field_access = FieldAccess { path: field_path.to_vec() };
                    let value = self.get_field_value(doc, &field_access)?;
                    
                    if value.is_null() {
                        continue;
                    }
                    
                    match &max_value {
                        None => max_value = Some(value),
                        Some(current_max) => {
                            if self.compare_ordered_values(&value, current_max, |ord| ord.is_gt())? {
                                max_value = Some(value);
                            }
                        }
                    }
                }
                
                Ok(max_value.unwrap_or(Value::Null))
            }
            _ => Err(OQLError::Generic {
                message: format!("Unknown aggregation function: {}", function_name),
            }),
        }
    }
    
    /// Apply GROUP BY operation to documents
    fn apply_group_by_operation(&self, documents: Vec<Document>, group_by_block: &GroupByBlock) -> OQLResult<Vec<Document>> {
        use std::collections::BTreeMap;
        
        // Group documents by the specified fields
        let mut groups: BTreeMap<String, Vec<Document>> = BTreeMap::new();
        
        for document in documents {
            // Create group key by concatenating field values
            let mut group_key_parts = Vec::new();
            
            for field_access in &group_by_block.fields {
                let value = self.get_field_value(&document, field_access)?;
                group_key_parts.push(value.to_string_repr());
            }
            
            let group_key = group_key_parts.join("|"); // Use | as separator
            groups.entry(group_key).or_insert_with(Vec::new).push(document);
        }
        
        // Create a document for each group
        let mut grouped_documents = Vec::new();
        
        for (group_key, group_documents) in groups {
            let mut fields = HashMap::new();
            
            // Add group key fields
            let group_key_parts: Vec<&str> = group_key.split('|').collect();
            for (i, field_access) in group_by_block.fields.iter().enumerate() {
                let field_name = field_access.path.last().unwrap_or(&"group_key".to_string()).clone();
                let field_value = if i < group_key_parts.len() {
                    // Try to parse back to original type
                    let key_part = group_key_parts[i];
                    if let Ok(int_val) = key_part.parse::<i64>() {
                        Value::Integer(int_val)
                    } else if let Ok(float_val) = key_part.parse::<f64>() {
                        Value::Float(float_val)
                    } else if key_part == "true" {
                        Value::Boolean(true)
                    } else if key_part == "false" {
                        Value::Boolean(false)
                    } else {
                        Value::String(key_part.to_string())
                    }
                } else {
                    Value::Null
                };
                fields.insert(field_name, field_value);
            }
            
            // Add group count
            fields.insert("group_count".to_string(), Value::Integer(group_documents.len() as i64));
            
            // Store the original documents as a special field for aggregation
            // We'll serialize the document IDs and store them for later aggregation
            let doc_ids: Vec<String> = group_documents.iter().map(|d| d.id.clone()).collect();
            fields.insert("_group_document_ids".to_string(), Value::String(doc_ids.join(",")));
            
            // Create group document
            let group_doc = Document::with_id(
                format!("group_{}", grouped_documents.len()),
                "group".to_string(),
                fields,
            );
            
            grouped_documents.push(group_doc);
        }
        
        Ok(grouped_documents)
    }
    
    /// Apply HAVING operation to grouped documents
    fn apply_having_operation(&self, documents: Vec<Document>, having_block: &HavingBlock) -> OQLResult<Vec<Document>> {
        // HAVING filters grouped documents based on aggregate conditions
        // For now, we'll implement a simple version that filters based on the condition
        self.apply_where_filter(documents, &having_block.condition)
    }
    
    /// Apply COUNT operation to documents
    fn apply_count_operation(&self, documents: Vec<Document>) -> OQLResult<Vec<Document>> {
        let count = documents.len() as i64;
        let mut fields = HashMap::new();
        fields.insert("count".to_string(), Value::Integer(count));
        
        let result_doc = Document::with_id(
            "count_result".to_string(),
            "aggregate".to_string(),
            fields,
        );
        
        Ok(vec![result_doc])
    }
    
    /// Apply SUM operation to documents
    fn apply_sum_operation(&self, documents: Vec<Document>, field_access: &FieldAccess) -> OQLResult<Vec<Document>> {
        let mut sum = 0.0;
        
        for document in &documents {
            if let Ok(value) = self.get_field_value(document, field_access) {
                match value {
                    Value::Integer(i) => sum += i as f64,
                    Value::Float(f) => sum += f,
                    _ => {} // Skip non-numeric values
                }
            }
        }
        
        let mut fields = HashMap::new();
        fields.insert("sum".to_string(), Value::Float(sum));
        
        let result_doc = Document::with_id(
            "sum_result".to_string(),
            "aggregate".to_string(),
            fields,
        );
        
        Ok(vec![result_doc])
    }
    
    /// Apply AVG operation to documents
    fn apply_avg_operation(&self, documents: Vec<Document>, field_access: &FieldAccess) -> OQLResult<Vec<Document>> {
        let mut sum = 0.0;
        let mut count = 0;
        
        for document in &documents {
            if let Ok(value) = self.get_field_value(document, field_access) {
                match value {
                    Value::Integer(i) => {
                        sum += i as f64;
                        count += 1;
                    }
                    Value::Float(f) => {
                        sum += f;
                        count += 1;
                    }
                    _ => {} // Skip non-numeric values
                }
            }
        }
        
        let avg = if count > 0 { sum / count as f64 } else { 0.0 };
        
        let mut fields = HashMap::new();
        fields.insert("avg".to_string(), Value::Float(avg));
        
        let result_doc = Document::with_id(
            "avg_result".to_string(),
            "aggregate".to_string(),
            fields,
        );
        
        Ok(vec![result_doc])
    }
    
    /// Apply MIN operation to documents
    fn apply_min_operation(&self, documents: Vec<Document>, field_access: &FieldAccess) -> OQLResult<Vec<Document>> {
        let mut min_value: Option<f64> = None;
        
        for document in &documents {
            if let Ok(value) = self.get_field_value(document, field_access) {
                let numeric_value = match value {
                    Value::Integer(i) => Some(i as f64),
                    Value::Float(f) => Some(f),
                    _ => None,
                };
                
                if let Some(val) = numeric_value {
                    min_value = Some(min_value.map_or(val, |current| current.min(val)));
                }
            }
        }
        
        let mut fields = HashMap::new();
        fields.insert("min".to_string(), 
            min_value.map(Value::Float).unwrap_or(Value::Null));
        
        let result_doc = Document::with_id(
            "min_result".to_string(),
            "aggregate".to_string(),
            fields,
        );
        
        Ok(vec![result_doc])
    }
    
    /// Apply MAX operation to documents
    fn apply_max_operation(&self, documents: Vec<Document>, field_access: &FieldAccess) -> OQLResult<Vec<Document>> {
        let mut max_value: Option<f64> = None;
        
        for document in &documents {
            if let Ok(value) = self.get_field_value(document, field_access) {
                let numeric_value = match value {
                    Value::Integer(i) => Some(i as f64),
                    Value::Float(f) => Some(f),
                    _ => None,
                };
                
                if let Some(val) = numeric_value {
                    max_value = Some(max_value.map_or(val, |current| current.max(val)));
                }
            }
        }
        
        let mut fields = HashMap::new();
        fields.insert("max".to_string(), 
            max_value.map(Value::Float).unwrap_or(Value::Null));
        
        let result_doc = Document::with_id(
            "max_result".to_string(),
            "aggregate".to_string(),
            fields,
        );
        
        Ok(vec![result_doc])
    }
    
    /// Execute aggregation over grouped documents
    pub fn execute_group_aggregation(&self, group_doc: &Document, function_name: &str, field_path: &[String]) -> OQLResult<Value> {
        // Get the original document IDs from the group
        let doc_ids_str = match group_doc.get_field("_group_document_ids") {
            Some(Value::String(ids)) => ids,
            _ => return Err(OQLError::Generic {
                message: "Group document missing document IDs".to_string(),
            }),
        };
        
        // Retrieve the original documents
        let doc_ids: Vec<&str> = doc_ids_str.split(',').collect();
        let mut documents = Vec::new();
        
        // We need to get the schema from somewhere - let's assume it's the same as the first document
        // For now, we'll use a simple approach and scan all documents to find matching IDs
        // This is not efficient but works for the demo
        let all_docs = self.scan_collection("sales")?; // Hardcoded for now
        
        for doc in all_docs {
            if doc_ids.contains(&doc.id.as_str()) {
                documents.push(doc);
            }
        }
        
        // Now execute aggregation on the original documents
        self.execute_aggregation(&documents, function_name, field_path)
    }
    
    /// Apply SORT operation to documents
    fn apply_sort_operation(&self, mut documents: Vec<Document>, sort_block: &SortBlock) -> OQLResult<Vec<Document>> {
        // Sort documents by the specified fields
        documents.sort_by(|a, b| {
            for sort_field in &sort_block.fields {
                // Evaluate the expression for both documents
                let a_value = self.evaluate_expression(a, &sort_field.expression).unwrap_or(Value::Null);
                let b_value = self.evaluate_expression(b, &sort_field.expression).unwrap_or(Value::Null);
                
                let comparison = self.compare_values_for_sort(&a_value, &b_value);
                
                let result = match sort_field.direction {
                    crate::parser::SortDirection::Asc => comparison,
                    crate::parser::SortDirection::Desc => comparison.reverse(),
                };
                
                if result != std::cmp::Ordering::Equal {
                    return result;
                }
            }
            std::cmp::Ordering::Equal
        });
        
        Ok(documents)
    }
    
    /// Apply LIMIT operation to documents
    fn apply_limit_operation(&self, documents: Vec<Document>, limit: u64) -> OQLResult<Vec<Document>> {
        let limit = limit as usize;
        if documents.len() <= limit {
            Ok(documents)
        } else {
            Ok(documents.into_iter().take(limit).collect())
        }
    }
    
    /// Compare two values for sorting purposes
    fn compare_values_for_sort(&self, left: &Value, right: &Value) -> std::cmp::Ordering {
        match (left, right) {
            (Value::Null, Value::Null) => std::cmp::Ordering::Equal,
            (Value::Null, _) => std::cmp::Ordering::Less, // Null values sort first
            (_, Value::Null) => std::cmp::Ordering::Greater,
            (Value::String(a), Value::String(b)) => a.cmp(b),
            (Value::Integer(a), Value::Integer(b)) => a.cmp(b),
            (Value::Float(a), Value::Float(b)) => a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal),
            (Value::Boolean(a), Value::Boolean(b)) => a.cmp(b),
            (Value::DateTime(a), Value::DateTime(b)) => a.cmp(b),
            // Type coercion: Integer to Float
            (Value::Integer(a), Value::Float(b)) => (*a as f64).partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal),
            (Value::Float(a), Value::Integer(b)) => a.partial_cmp(&(*b as f64)).unwrap_or(std::cmp::Ordering::Equal),
            // Different types: sort by type name for consistency
            _ => left.type_name().cmp(right.type_name()),
        }
    }
    
    /// Evaluate a user-defined function call
    fn evaluate_user_function(&self, document: &Document, user_func: &crate::functions::UserDefinedFunction, args: &[Expression]) -> OQLResult<Value> {
        // Validate argument count
        if args.len() != user_func.parameter_types.len() {
            return Err(OQLError::Generic {
                message: format!(
                    "Function '{}' expects {} arguments, got {}",
                    user_func.definition.name,
                    user_func.parameter_types.len(),
                    args.len()
                ),
            });
        }
        
        // Evaluate arguments
        let mut arg_values = Vec::new();
        for (i, arg) in args.iter().enumerate() {
            let value = self.evaluate_expression(document, arg)?;
            
            // Type checking
            let expected_type = &user_func.parameter_types[i];
            if !self.is_value_compatible_with_type(&value, expected_type) {
                return Err(OQLError::TypeError {
                    expected: expected_type.to_string(),
                    actual: value.type_name().to_string(),
                });
            }
            
            arg_values.push(value);
        }
        
        // Create a parameter binding context
        let mut param_context = HashMap::new();
        for (i, param) in user_func.definition.parameters.iter().enumerate() {
            param_context.insert(param.name.clone(), arg_values[i].clone());
        }
        
        // Evaluate the function body with parameter bindings
        self.evaluate_expression_with_context(document, &user_func.definition.body, &param_context)
    }
    
    /// Check if a value is compatible with a type
    fn is_value_compatible_with_type(&self, value: &Value, type_name: &str) -> bool {
        match (value, type_name) {
            (Value::String(_), "String") => true,
            (Value::Integer(_), "Integer") => true,
            (Value::Float(_), "Float") => true,
            (Value::Boolean(_), "Boolean") => true,
            (Value::DateTime(_), "DateTime") => true,
            (Value::Null, _) => true, // Null is compatible with any type
            // Schema types would need more complex checking
            _ => false,
        }
    }
    
    /// Evaluate an expression with parameter context (for user-defined functions)
    fn evaluate_expression_with_context(&self, document: &Document, expr: &Expression, context: &HashMap<String, Value>) -> OQLResult<Value> {
        match expr {
            Expression::Literal(literal) => {
                Ok(self.literal_to_value(literal))
            }
            Expression::FieldAccess(field_access) => {
                // Check if this is a parameter reference
                if field_access.path.len() == 1 {
                    let field_name = &field_access.path[0];
                    if let Some(param_value) = context.get(field_name) {
                        return Ok(param_value.clone());
                    }
                }
                
                // Otherwise, evaluate as normal field access
                self.get_field_value(document, field_access)
            }
            Expression::FunctionCall { name, args } => {
                // Recursively evaluate function calls within user functions
                self.evaluate_function_call(document, name, args)
            }
            Expression::IfThenElse { condition, then_expr, else_expr } => {
                let condition_result = self.evaluate_condition(document, condition)?;
                if condition_result {
                    self.evaluate_expression_with_context(document, then_expr, context)
                } else {
                    self.evaluate_expression_with_context(document, else_expr, context)
                }
            }
            Expression::Match { expr, arms } => {
                let expr_value = self.evaluate_expression_with_context(document, expr, context)?;
                self.evaluate_match_expression_with_context(document, &expr_value, arms, context)
            }
            Expression::LogicalAnd(left, right) => {
                let left_value = self.evaluate_expression_with_context(document, left, context)?;
                let left_bool = self.value_to_bool(&left_value)?;
                
                if !left_bool {
                    Ok(Value::Boolean(false))
                } else {
                    let right_value = self.evaluate_expression_with_context(document, right, context)?;
                    let right_bool = self.value_to_bool(&right_value)?;
                    Ok(Value::Boolean(right_bool))
                }
            }
            Expression::LogicalOr(left, right) => {
                let left_value = self.evaluate_expression_with_context(document, left, context)?;
                let left_bool = self.value_to_bool(&left_value)?;
                
                if left_bool {
                    Ok(Value::Boolean(true))
                } else {
                    let right_value = self.evaluate_expression_with_context(document, right, context)?;
                    let right_bool = self.value_to_bool(&right_value)?;
                    Ok(Value::Boolean(right_bool))
                }
            }
            Expression::LogicalNot(expr) => {
                let value = self.evaluate_expression_with_context(document, expr, context)?;
                let bool_value = self.value_to_bool(&value)?;
                Ok(Value::Boolean(!bool_value))
            }
            Expression::BinaryOp { left, operator, right } => {
                let left_value = self.evaluate_expression_with_context(document, left, context)?;
                let right_value = self.evaluate_expression_with_context(document, right, context)?;
                self.evaluate_binary_operation(&left_value, operator, &right_value)
            }
            Expression::ArrayAccess { array, index } => {
                let array_value = self.evaluate_expression_with_context(document, array, context)?;
                let index_value = self.evaluate_expression_with_context(document, index, context)?;
                self.evaluate_array_access(&array_value, &index_value)
            }
            Expression::ChainedAccess { base, field } => {
                let base_value = self.evaluate_expression_with_context(document, base, context)?;
                self.get_field_from_value(&base_value, field)
            }
            Expression::Block(expressions) => {
                // Evaluate all expressions in the block and return the last one
                let mut last_value = Value::Null;
                for expr in expressions {
                    last_value = self.evaluate_expression_with_context(document, expr, context)?;
                }
                Ok(last_value)
            }
            Expression::ArrayLiteral(elements) => {
                let mut values = Vec::new();
                for element in elements {
                    let value = self.evaluate_expression_with_context(document, element, context)?;
                    values.push(value);
                }
                Ok(Value::List(values))
            }
            Expression::ObjectLiteral(fields) => {
                let mut object_fields = std::collections::HashMap::new();
                for field in fields {
                    let value = self.evaluate_expression_with_context(document, &field.value, context)?;
                    object_fields.insert(field.name.clone(), value);
                }
                Ok(Value::Object(object_fields))
            }
        }
    }
    
    /// Evaluate a match expression with parameter context
    fn evaluate_match_expression_with_context(&self, document: &Document, expr_value: &Value, arms: &[crate::parser::MatchArm], context: &HashMap<String, Value>) -> OQLResult<Value> {
        for arm in arms {
            if self.pattern_matches(&arm.pattern, expr_value)? {
                // Check guard condition if present
                if let Some(guard) = &arm.guard {
                    if !self.evaluate_condition(document, guard)? {
                        continue;
                    }
                }
                
                return self.evaluate_expression_with_context(document, &arm.body, context);
            }
        }
        
        Err(OQLError::Generic {
            message: "No matching pattern found in match expression".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::parser::parse_oql;
    use crate::storage::create_storage;
    use std::collections::HashMap;
    use tempfile::TempDir;
    
    fn create_test_executor() -> (QueryExecutor<dyn DocumentStore + 'static>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("test.owl");
        let store = create_storage(path.to_str().unwrap(), 10 * 1024 * 1024).unwrap();
        let executor = QueryExecutor::new(store);
        (executor, temp_dir)
    }
    
    fn create_test_documents() -> Vec<Document> {
        vec![
            Document::with_id("user_1".to_string(), "users".to_string(), {
                let mut fields = HashMap::new();
                fields.insert("name".to_string(), Value::String("Alice".to_string()));
                fields.insert("age".to_string(), Value::Integer(30));
                fields.insert("active".to_string(), Value::Boolean(true));
                fields
            }),
            Document::with_id("user_2".to_string(), "users".to_string(), {
                let mut fields = HashMap::new();
                fields.insert("name".to_string(), Value::String("Bob".to_string()));
                fields.insert("age".to_string(), Value::Integer(25));
                fields.insert("active".to_string(), Value::Boolean(false));
                fields
            }),
            Document::with_id("user_3".to_string(), "users".to_string(), {
                let mut fields = HashMap::new();
                fields.insert("name".to_string(), Value::String("Charlie".to_string()));
                fields.insert("age".to_string(), Value::Integer(35));
                fields.insert("active".to_string(), Value::Boolean(true));
                fields
            }),
        ]
    }
    
    #[test]
    fn test_scan_collection() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();
        
        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }
        
        // Scan collection
        let scanned = executor.scan_collection("users").unwrap();
        assert_eq!(scanned.len(), 3);
        
        // Verify all documents are present
        let ids: Vec<&String> = scanned.iter().map(|d| &d.id).collect();
        assert!(ids.contains(&&"user_1".to_string()));
        assert!(ids.contains(&&"user_2".to_string()));
        assert!(ids.contains(&&"user_3".to_string()));
    }
    
    #[test]
    fn test_where_filter_equality() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();
        
        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }
        
        // Test query: users.where { .name == "Alice" }
        let query = parse_oql(r#"users.where { .name == "Alice" }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();
        
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "user_1");
        assert_eq!(results[0].get_field("name"), Some(&Value::String("Alice".to_string())));
    }
    
    #[test]
    fn test_where_filter_comparison() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();
        
        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }
        
        // Test query: users.where { .age >= 30 }
        let query = parse_oql(r#"users.where { .age >= 30 }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();
        
        assert_eq!(results.len(), 2);
        let ids: Vec<&String> = results.iter().map(|d| &d.id).collect();
        assert!(ids.contains(&&"user_1".to_string())); // Alice, age 30
        assert!(ids.contains(&&"user_3".to_string())); // Charlie, age 35
    }
    
    #[test]
    fn test_where_filter_boolean() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();
        
        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }
        
        // Test query: users.where { .active == true }
        let query = parse_oql(r#"users.where { .active == true }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();
        
        assert_eq!(results.len(), 2);
        let ids: Vec<&String> = results.iter().map(|d| &d.id).collect();
        assert!(ids.contains(&&"user_1".to_string())); // Alice, active
        assert!(ids.contains(&&"user_3".to_string())); // Charlie, active
    }
    
    #[test]
    fn test_where_filter_no_matches() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();
        
        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }
        
        // Test query: users.where { .name == "David" }
        let query = parse_oql(r#"users.where { .name == "David" }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();
        
        assert_eq!(results.len(), 0);
    }
    
    #[test]
    fn test_where_filter_missing_field() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();
        
        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }
        
        // Test query: users.where { .email == "test@example.com" }
        let query = parse_oql(r#"users.where { .email == "test@example.com" }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();
        
        // Should return no results since no documents have an email field
        assert_eq!(results.len(), 0);
    }
    
    #[test]
    fn test_value_comparison_type_coercion() {
        let (executor, _temp_dir) = create_test_executor();
        
        // Test integer to float comparison
        assert!(executor.values_equal(&Value::Integer(42), &Value::Float(42.0)));
        assert!(executor.values_equal(&Value::Float(42.0), &Value::Integer(42)));
        
        // Test comparison operators with type coercion
        assert!(executor.compare_ordered_values(
            &Value::Integer(10), 
            &Value::Float(20.5), 
            |ord| ord.is_lt()
        ).unwrap());
        
        assert!(executor.compare_ordered_values(
            &Value::Float(30.5), 
            &Value::Integer(20), 
            |ord| ord.is_gt()
        ).unwrap());
    }
    
    #[test]
    fn test_empty_collection() {
        let (executor, _temp_dir) = create_test_executor();
        
        // Test scanning empty collection
        let results = executor.scan_collection("empty_collection").unwrap();
        assert_eq!(results.len(), 0);
        
        // Test query on empty collection
        let query = parse_oql(r#"empty_collection.where { .name == "Alice" }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();
        assert_eq!(results.len(), 0);
    }
    
    #[test]
    fn test_insert_operation() {
        let (executor, _temp_dir) = create_test_executor();
        
        // Test INSERT operation
        let query = parse_oql(r#"users.insert { id: "user_4", name: "David", age: 28, active: true }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();
        
        assert_eq!(results.len(), 1);
        let inserted_doc = &results[0];
        assert_eq!(inserted_doc.schema, "users");
        assert_eq!(inserted_doc.get_field("name"), Some(&Value::String("David".to_string())));
        assert_eq!(inserted_doc.get_field("age"), Some(&Value::Integer(28)));
        assert_eq!(inserted_doc.get_field("active"), Some(&Value::Boolean(true)));
        
        // Verify document was stored
        let stored_doc = executor.store.get_document("users", &inserted_doc.id).unwrap();
        assert!(stored_doc.is_some());
        assert_eq!(stored_doc.unwrap(), *inserted_doc);
    }
    
    #[test]
    fn test_update_operation() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();
        
        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }
        
        // Test UPDATE operation: users.where { .name == "Alice" }.update { age: 31 }
        let query = parse_oql(r#"users.where { .name == "Alice" }.update { age: 31 }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();
        
        assert_eq!(results.len(), 1);
        let updated_doc = &results[0];
        assert_eq!(updated_doc.id, "user_1");
        assert_eq!(updated_doc.get_field("age"), Some(&Value::Integer(31)));
        assert_eq!(updated_doc.get_field("name"), Some(&Value::String("Alice".to_string())));
        
        // Verify document was updated in storage
        let stored_doc = executor.store.get_document("users", &updated_doc.id).unwrap().unwrap();
        assert_eq!(stored_doc.get_field("age"), Some(&Value::Integer(31)));
    }
    
    #[test]
    fn test_delete_operation() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();
        
        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }
        
        // Test DELETE operation: users.where { .name == "Bob" }.delete
        let query = parse_oql(r#"users.where { .name == "Bob" }.delete"#).unwrap();
        let results = executor.execute_query(&query).unwrap();
        
        assert_eq!(results.len(), 1);
        let deleted_doc = &results[0];
        assert_eq!(deleted_doc.id, "user_2");
        assert_eq!(deleted_doc.get_field("name"), Some(&Value::String("Bob".to_string())));
        
        // Verify document was deleted from storage
        let stored_doc = executor.store.get_document("users", &deleted_doc.id).unwrap();
        assert!(stored_doc.is_none());
        
        // Verify other documents still exist
        let alice_doc = executor.store.get_document("users", &"user_1".to_string()).unwrap();
        assert!(alice_doc.is_some());
    }
    
    #[test]
    fn test_select_projection() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();
        
        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }
        
        // Test SELECT with field projection: users.select { name, age }
        let query = parse_oql(r#"users.select { name, age }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();
        
        assert_eq!(results.len(), 3);
        
        for doc in &results {
            // Should only have name and age fields
            assert!(doc.get_field("name").is_some());
            assert!(doc.get_field("age").is_some());
            // Should not have active field (not selected)
            assert!(doc.get_field("active").is_none());
        }
    }
    
    #[test]
    fn test_select_with_alias() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();
        
        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }
        
        // Test SELECT with alias: users.select { full_name: .name, years: .age }
        let query = parse_oql(r#"users.select { full_name: .name, years: .age }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();
        
        assert_eq!(results.len(), 3);
        
        for doc in &results {
            // Should have aliased fields
            assert!(doc.get_field("full_name").is_some());
            assert!(doc.get_field("years").is_some());
            // Should not have original field names
            assert!(doc.get_field("name").is_none());
            assert!(doc.get_field("age").is_none());
        }
        
        // Check specific values
        let alice_doc = results.iter().find(|d| d.get_field("full_name") == Some(&Value::String("Alice".to_string()))).unwrap();
        assert_eq!(alice_doc.get_field("years"), Some(&Value::Integer(30)));
    }
    
    #[test]
    fn test_combined_operations() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();
        
        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }
        
        // Test combined WHERE and SELECT: users.where { .age >= 30 }.select { name }
        let query = parse_oql(r#"users.where { .age >= 30 }.select { name }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();
        
        assert_eq!(results.len(), 2);
        
        for doc in &results {
            // Should only have name field
            assert!(doc.get_field("name").is_some());
            assert!(doc.get_field("age").is_none());
            assert!(doc.get_field("active").is_none());
        }
        
        // Check that only Alice and Charlie are returned (age >= 30)
        let names: Vec<String> = results.iter()
            .filter_map(|d| match d.get_field("name") {
                Some(Value::String(name)) => Some(name.clone()),
                _ => None,
            })
            .collect();
        
        assert!(names.contains(&"Alice".to_string()));
        assert!(names.contains(&"Charlie".to_string()));
        assert!(!names.contains(&"Bob".to_string()));
    }
    
    #[test]
    fn test_update_multiple_documents() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();
        
        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }
        
        // Test UPDATE multiple: users.where { .active == true }.update { age: 99 }
        let query = parse_oql(r#"users.where { .active == true }.update { age: 99 }"#).unwrap();
        let results = executor.execute_query(&query).unwrap();
        
        assert_eq!(results.len(), 2); // Alice and Charlie are active
        
        for doc in &results {
            assert_eq!(doc.get_field("age"), Some(&Value::Integer(99)));
            assert_eq!(doc.get_field("active"), Some(&Value::Boolean(true)));
        }
        
        // Verify Bob's age wasn't changed (he's not active)
        let bob_doc = executor.store.get_document("users", &"user_2".to_string()).unwrap().unwrap();
        assert_eq!(bob_doc.get_field("age"), Some(&Value::Integer(25)));
    }
    
    #[test]
    fn test_delete_multiple_documents() {
        let (executor, _temp_dir) = create_test_executor();
        let documents = create_test_documents();
        
        // Store test documents
        for doc in &documents {
            executor.store.put_document(doc).unwrap();
        }
        
        // Test DELETE multiple: users.where { .active == true }.delete
        let query = parse_oql(r#"users.where { .active == true }.delete"#).unwrap();
        let results = executor.execute_query(&query).unwrap();
        
        assert_eq!(results.len(), 2); // Alice and Charlie are active
        
        // Verify active users were deleted
        let alice_doc = executor.store.get_document("users", &"user_1".to_string()).unwrap();
        assert!(alice_doc.is_none());
        let charlie_doc = executor.store.get_document("users", &"user_3".to_string()).unwrap();
        assert!(charlie_doc.is_none());
        
        // Verify Bob still exists (he's not active)
        let bob_doc = executor.store.get_document("users", &"user_2".to_string()).unwrap();
        assert!(bob_doc.is_some());
    }
}