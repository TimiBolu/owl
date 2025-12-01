// Logical Query Planner
// Converts AST to logical query plan with type checking and schema validation

use crate::error::{OQLError, OQLResult};
use crate::parser::{Expression, FieldAccess, Literal, OQLQuery, Operation};
use crate::planner::types::*;
use crate::schema::{FieldType, SchemaRegistry};
use std::collections::HashMap;

/// Logical query plan
#[derive(Debug, Clone)]
pub struct LogicalPlan {
    /// Root node of the plan tree
    pub root: PlanNode,
    /// Schema registry for type checking
    pub schema_registry: Option<SchemaRegistry>,
}

/// Logical query planner
pub struct LogicalPlanner {
    /// Schema registry for validation
    schema_registry: Option<SchemaRegistry>,
}

impl LogicalPlanner {
    /// Create a new logical planner
    pub fn new() -> Self {
        Self {
            schema_registry: None,
        }
    }
    
    /// Create a planner with schema registry
    pub fn with_schema_registry(schema_registry: SchemaRegistry) -> Self {
        Self {
            schema_registry: Some(schema_registry),
        }
    }
    
    /// Convert an OQL query to a logical plan
    pub fn plan(&self, query: &OQLQuery) -> OQLResult<LogicalPlan> {
        // Start with a scan of the collection
        let mut plan = PlanNode::new(PlanType::Scan)
            .with_schema(query.collection.clone());
        
        // Validate schema exists
        if let Some(registry) = &self.schema_registry {
            if !registry.exists(&query.collection) {
                return Err(OQLError::SchemaValidation {
                    message: format!("Schema '{}' not found", query.collection),
                });
            }
        }
        
        // Apply operations in sequence
        for operation in &query.operations {
            plan = self.plan_operation(plan, operation, &query.collection)?;
        }
        
        Ok(LogicalPlan {
            root: plan,
            schema_registry: self.schema_registry.clone(),
        })
    }
    
    /// Plan a single operation
    fn plan_operation(
        &self,
        input: PlanNode,
        operation: &Operation,
        schema: &str,
    ) -> OQLResult<PlanNode> {
        match operation {
            Operation::Where(where_block) => {
                // Validate condition types
                self.validate_condition(&where_block.condition, schema)?;
                
                let condition = FilterCondition::from_parser_condition(&where_block.condition);
                Ok(PlanNode::new(PlanType::Filter)
                    .with_child(input)
                    .with_condition(condition))
            }
            
            Operation::Select(select_block) => {
                let mut projections = Vec::new();
                
                for field in &select_block.fields {
                    // Infer type for the expression
                    let field_type = self.infer_expression_type(&field.expression, schema)?;
                    
                    projections.push(ProjectionField {
                        expression: field.expression.clone(),
                        alias: field.alias.clone(),
                        field_type: Some(field_type),
                    });
                }
                
                Ok(PlanNode::new(PlanType::Project)
                    .with_child(input)
                    .with_projections(projections))
            }
            
            Operation::Sort(sort_block) => {
                let mut sort_specs = Vec::new();
                
                for sort_field in &sort_block.fields {
                    // Validate sort expression type
                    let field_type = self.infer_expression_type(&sort_field.expression, schema)?;
                    
                    // Check if type is sortable
                    if !self.is_sortable_type(&field_type) {
                        return Err(OQLError::TypeError {
                            expected: "sortable type".to_string(),
                            actual: field_type.type_name().to_string(),
                        });
                    }
                    
                    sort_specs.push(SortSpec {
                        expression: sort_field.expression.clone(),
                        direction: sort_field.direction.clone(),
                    });
                }
                
                Ok(PlanNode::new(PlanType::Sort)
                    .with_child(input)
                    .with_sort_specs(sort_specs))
            }
            
            Operation::Limit(limit) => {
                Ok(PlanNode::new(PlanType::Limit)
                    .with_child(input)
                    .with_limit(*limit))
            }
            
            Operation::GroupBy(group_by_block) => {
                let fields: Vec<String> = group_by_block.fields
                    .iter()
                    .flat_map(|f| f.path.clone())
                    .collect();
                
                // Validate group by fields exist
                for field in &fields {
                    self.validate_field_exists(schema, field)?;
                }
                
                Ok(PlanNode::new(PlanType::GroupBy)
                    .with_child(input)
                    .with_group_by(fields))
            }
            
            Operation::Having(having_block) => {
                // Validate having condition
                self.validate_condition(&having_block.condition, schema)?;
                
                let condition = FilterCondition::from_parser_condition(&having_block.condition);
                Ok(PlanNode::new(PlanType::Having)
                    .with_child(input)
                    .with_condition(condition))
            }
            
            Operation::Count => {
                let aggregates = vec![AggregateSpec {
                    function: AggregateFunction::Count,
                    field: None,
                    alias: Some("count".to_string()),
                }];
                
                Ok(PlanNode::new(PlanType::Aggregate)
                    .with_child(input)
                    .with_aggregates(aggregates))
            }
            
            Operation::Sum(field_access) => {
                self.validate_aggregate_field(schema, field_access, "sum")?;
                
                let aggregates = vec![AggregateSpec {
                    function: AggregateFunction::Sum,
                    field: Some(field_access.clone()),
                    alias: Some("sum".to_string()),
                }];
                
                Ok(PlanNode::new(PlanType::Aggregate)
                    .with_child(input)
                    .with_aggregates(aggregates))
            }
            
            Operation::Avg(field_access) => {
                self.validate_aggregate_field(schema, field_access, "avg")?;
                
                let aggregates = vec![AggregateSpec {
                    function: AggregateFunction::Avg,
                    field: Some(field_access.clone()),
                    alias: Some("avg".to_string()),
                }];
                
                Ok(PlanNode::new(PlanType::Aggregate)
                    .with_child(input)
                    .with_aggregates(aggregates))
            }
            
            Operation::Min(field_access) => {
                self.validate_aggregate_field(schema, field_access, "min")?;
                
                let aggregates = vec![AggregateSpec {
                    function: AggregateFunction::Min,
                    field: Some(field_access.clone()),
                    alias: Some("min".to_string()),
                }];
                
                Ok(PlanNode::new(PlanType::Aggregate)
                    .with_child(input)
                    .with_aggregates(aggregates))
            }
            
            Operation::Max(field_access) => {
                self.validate_aggregate_field(schema, field_access, "max")?;
                
                let aggregates = vec![AggregateSpec {
                    function: AggregateFunction::Max,
                    field: Some(field_access.clone()),
                    alias: Some("max".to_string()),
                }];
                
                Ok(PlanNode::new(PlanType::Aggregate)
                    .with_child(input)
                    .with_aggregates(aggregates))
            }
            
            Operation::Insert(insert_block) => {
                let mut field_updates = HashMap::new();
                
                for field in &insert_block.fields {
                    // Validate field type if schema is available
                    if let Some(registry) = &self.schema_registry {
                        let expr_type = self.infer_expression_type(&field.value, schema)?;
                        if let Ok(Some(field_type)) = registry.get_field_type(schema, &field.name) {
                            self.validate_type_compatibility(&expr_type, &field_type)?;
                        }
                    }
                    
                    field_updates.insert(field.name.clone(), field.value.clone());
                }
                
                Ok(PlanNode::new(PlanType::Insert)
                    .with_schema(schema.to_string())
                    .with_field_updates(field_updates))
            }
            
            Operation::InsertMany(insert_blocks) => {
                // For batch insert, validate all blocks
                for insert_block in insert_blocks {
                    for field in &insert_block.fields {
                        // Validate field type if schema is available
                        if let Some(registry) = &self.schema_registry {
                            let expr_type = self.infer_expression_type(&field.value, schema)?;
                            if let Ok(Some(field_type)) = registry.get_field_type(schema, &field.name) {
                                self.validate_type_compatibility(&expr_type, &field_type)?;
                            }
                        }
                    }
                }
                
                // For planning purposes, treat as a batch insert operation
                // The actual execution will handle the batch
                Ok(PlanNode::new(PlanType::Insert)
                    .with_schema(schema.to_string()))
            }
            
            Operation::Update(update_block) => {
                let mut field_updates = HashMap::new();
                
                for field in &update_block.fields {
                    // Validate field type if schema is available
                    if let Some(registry) = &self.schema_registry {
                        let expr_type = self.infer_expression_type(&field.value, schema)?;
                        if let Ok(Some(field_type)) = registry.get_field_type(schema, &field.name) {
                            self.validate_type_compatibility(&expr_type, &field_type)?;
                        }
                    }
                    
                    field_updates.insert(field.name.clone(), field.value.clone());
                }
                
                Ok(PlanNode::new(PlanType::Update)
                    .with_child(input)
                    .with_field_updates(field_updates))
            }
            
            Operation::Delete => {
                Ok(PlanNode::new(PlanType::Delete)
                    .with_child(input))
            }
            
            Operation::FunctionDef(_) => {
                Err(OQLError::Generic {
                    message: "Function definitions cannot be part of query plan".to_string(),
                })
            }
        }
    }
    
    /// Validate a condition's types
    fn validate_condition(&self, condition: &crate::parser::Condition, schema: &str) -> OQLResult<()> {
        match condition {
            crate::parser::Condition::Comparison { left, operator, right } => {
                let left_type = self.infer_expression_type(left, schema)?;
                let right_type = self.infer_expression_type(right, schema)?;
                
                // Validate operator is valid for these types
                self.validate_comparison_operator(&left_type, operator)?;
                
                // Validate types are compatible
                self.validate_type_compatibility(&left_type, &right_type)?;
                
                Ok(())
            }
            crate::parser::Condition::And(left, right) | crate::parser::Condition::Or(left, right) => {
                self.validate_condition(left, schema)?;
                self.validate_condition(right, schema)?;
                Ok(())
            }
            crate::parser::Condition::Not(inner) => {
                self.validate_condition(inner, schema)
            }
        }
    }
    
    /// Infer the type of an expression
    fn infer_expression_type(&self, expr: &Expression, schema: &str) -> OQLResult<FieldType> {
        match expr {
            Expression::Literal(lit) => Ok(self.literal_type(lit)),
            
            Expression::FieldAccess(field_access) => {
                if let Some(registry) = &self.schema_registry {
                    let mut current_schema = schema.to_string();
                    let mut current_type: Option<FieldType> = None;
                    
                    for field_name in &field_access.path {
                        let field_type = registry.get_field_type(&current_schema, field_name)?
                            .ok_or_else(|| OQLError::SchemaValidation {
                                message: format!("Field '{}' not found in schema '{}'", field_name, current_schema),
                            })?;
                        
                        // If this is a reference, update current_schema for next iteration
                        if let Some(target_schema) = field_type.referenced_schema() {
                            current_schema = target_schema.to_string();
                        }
                        
                        current_type = Some(field_type);
                    }
                    
                    current_type.ok_or_else(|| OQLError::SchemaValidation {
                        message: "Empty field path".to_string(),
                    })
                } else {
                    // No schema registry - return generic type
                    Ok(FieldType::String)
                }
            }
            
            Expression::FunctionCall { name, args } => {
                self.infer_function_return_type(name, args, schema)
            }
            
            Expression::BinaryOp { left, operator, .. } => {
                let left_type = self.infer_expression_type(left, schema)?;
                
                match operator {
                    crate::parser::BinaryOperator::Add |
                    crate::parser::BinaryOperator::Subtract |
                    crate::parser::BinaryOperator::Multiply |
                    crate::parser::BinaryOperator::Divide |
                    crate::parser::BinaryOperator::Modulo => {
                        // Arithmetic operations preserve numeric types
                        Ok(left_type)
                    }
                }
            }
            
            Expression::IfThenElse { then_expr, .. } => {
                // Return type is the type of the then branch
                self.infer_expression_type(then_expr, schema)
            }
            
            Expression::LogicalAnd(..) | Expression::LogicalOr(..) | Expression::LogicalNot(..) => {
                Ok(FieldType::Boolean)
            }
            
            Expression::ArrayLiteral(elements) => {
                if elements.is_empty() {
                    Ok(FieldType::List(Box::new(FieldType::String)))
                } else {
                    let elem_type = self.infer_expression_type(&elements[0], schema)?;
                    Ok(FieldType::List(Box::new(elem_type)))
                }
            }
            
            Expression::ObjectLiteral(_) => {
                Ok(FieldType::String) // Simplified - would need nested type inference
            }
            
            _ => Ok(FieldType::String), // Default fallback
        }
    }
    
    /// Get the type of a literal
    fn literal_type(&self, lit: &Literal) -> FieldType {
        match lit {
            Literal::String(_) => FieldType::String,
            Literal::Integer(_) => FieldType::Integer,
            Literal::Float(_) => FieldType::Float,
            Literal::Boolean(_) => FieldType::Boolean,
        }
    }
    
    /// Infer function return type
    fn infer_function_return_type(&self, name: &str, args: &[Expression], schema: &str) -> OQLResult<FieldType> {
        match name {
            "count" => Ok(FieldType::Integer),
            "sum" | "avg" => {
                if args.is_empty() {
                    Ok(FieldType::Float)
                } else {
                    self.infer_expression_type(&args[0], schema)
                }
            }
            "min" | "max" => {
                if args.is_empty() {
                    Err(OQLError::TypeError {
                        expected: "at least one argument".to_string(),
                        actual: "no arguments".to_string(),
                    })
                } else {
                    self.infer_expression_type(&args[0], schema)
                }
            }
            "length" | "len" => Ok(FieldType::Integer),
            "upper" | "lower" | "trim" => Ok(FieldType::String),
            "now" => Ok(FieldType::DateTime),
            _ => Err(OQLError::FunctionError {
                message: format!("Unknown function '{}'", name),
            }),
        }
    }
    
    /// Validate that a comparison operator is valid for a type
    fn validate_comparison_operator(
        &self,
        field_type: &FieldType,
        operator: &crate::parser::ComparisonOperator,
    ) -> OQLResult<()> {
        use crate::parser::ComparisonOperator;
        
        match (field_type, operator) {
            // String operators
            (FieldType::String, ComparisonOperator::Contains) => Ok(()),
            (FieldType::String, ComparisonOperator::StartsWith) => Ok(()),
            (FieldType::String, ComparisonOperator::EndsWith) => Ok(()),
            (FieldType::String, ComparisonOperator::Equal) => Ok(()),
            (FieldType::String, ComparisonOperator::NotEqual) => Ok(()),
            
            // Numeric operators
            (FieldType::Integer, ComparisonOperator::LessThan) => Ok(()),
            (FieldType::Integer, ComparisonOperator::GreaterThan) => Ok(()),
            (FieldType::Integer, ComparisonOperator::LessEqual) => Ok(()),
            (FieldType::Integer, ComparisonOperator::GreaterEqual) => Ok(()),
            (FieldType::Integer, ComparisonOperator::Equal) => Ok(()),
            (FieldType::Integer, ComparisonOperator::NotEqual) => Ok(()),
            
            (FieldType::Float, ComparisonOperator::LessThan) => Ok(()),
            (FieldType::Float, ComparisonOperator::GreaterThan) => Ok(()),
            (FieldType::Float, ComparisonOperator::LessEqual) => Ok(()),
            (FieldType::Float, ComparisonOperator::GreaterEqual) => Ok(()),
            (FieldType::Float, ComparisonOperator::Equal) => Ok(()),
            (FieldType::Float, ComparisonOperator::NotEqual) => Ok(()),
            
            // Boolean operators
            (FieldType::Boolean, ComparisonOperator::Equal) => Ok(()),
            (FieldType::Boolean, ComparisonOperator::NotEqual) => Ok(()),
            
            // DateTime operators
            (FieldType::DateTime, ComparisonOperator::LessThan) => Ok(()),
            (FieldType::DateTime, ComparisonOperator::GreaterThan) => Ok(()),
            (FieldType::DateTime, ComparisonOperator::LessEqual) => Ok(()),
            (FieldType::DateTime, ComparisonOperator::GreaterEqual) => Ok(()),
            (FieldType::DateTime, ComparisonOperator::Equal) => Ok(()),
            (FieldType::DateTime, ComparisonOperator::NotEqual) => Ok(()),
            
            // Reference operators
            (FieldType::Reference(_), ComparisonOperator::Equal) => Ok(()),
            (FieldType::Reference(_), ComparisonOperator::NotEqual) => Ok(()),
            
            // List operators
            (FieldType::List(_), ComparisonOperator::Contains) => Ok(()),
            (FieldType::List(_), ComparisonOperator::Equal) => Ok(()),
            (FieldType::List(_), ComparisonOperator::NotEqual) => Ok(()),
            
            // Optional types - check inner type
            (FieldType::Optional(inner), op) => self.validate_comparison_operator(inner.as_ref(), op),
            
            // Invalid combinations
            _ => Err(OQLError::TypeError {
                expected: format!("type compatible with operator {:?}", operator),
                actual: field_type.type_name().to_string(),
            }),
        }
    }
    
    /// Validate that two types are compatible
    fn validate_type_compatibility(&self, left: &FieldType, right: &FieldType) -> OQLResult<()> {
        match (left, right) {
            // Same types are always compatible
            (FieldType::String, FieldType::String) => Ok(()),
            (FieldType::Integer, FieldType::Integer) => Ok(()),
            (FieldType::Float, FieldType::Float) => Ok(()),
            (FieldType::Boolean, FieldType::Boolean) => Ok(()),
            (FieldType::DateTime, FieldType::DateTime) => Ok(()),
            
            // Integer and Float are compatible
            (FieldType::Integer, FieldType::Float) => Ok(()),
            (FieldType::Float, FieldType::Integer) => Ok(()),
            
            // References to same schema are compatible
            (FieldType::Reference(ref1), FieldType::Reference(ref2))
                if ref1.target_schema == ref2.target_schema => Ok(()),
            
            // Optional types - check inner compatibility
            (FieldType::Optional(inner1), FieldType::Optional(inner2)) => {
                self.validate_type_compatibility(inner1, inner2)
            }
            (FieldType::Optional(inner), other) | (other, FieldType::Optional(inner)) => {
                self.validate_type_compatibility(inner, other)
            }
            
            // List types - check inner compatibility
            (FieldType::List(inner1), FieldType::List(inner2)) => {
                self.validate_type_compatibility(inner1, inner2)
            }
            
            // Incompatible types
            _ => Err(OQLError::TypeError {
                expected: left.type_name().to_string(),
                actual: right.type_name().to_string(),
            }),
        }
    }
    
    /// Check if a type is sortable
    fn is_sortable_type(&self, field_type: &FieldType) -> bool {
        matches!(
            field_type,
            FieldType::String | FieldType::Integer | FieldType::Float | FieldType::DateTime
        )
    }
    
    /// Validate that a field exists in the schema
    fn validate_field_exists(&self, schema: &str, field: &str) -> OQLResult<()> {
        if let Some(registry) = &self.schema_registry {
            registry.get_field_type(schema, field)?
                .ok_or_else(|| OQLError::SchemaValidation {
                    message: format!("Field '{}' not found in schema '{}'", field, schema),
                })?;
        }
        Ok(())
    }
    
    /// Validate an aggregate field
    fn validate_aggregate_field(
        &self,
        schema: &str,
        field_access: &FieldAccess,
        function: &str,
    ) -> OQLResult<()> {
        let field_type = self.infer_expression_type(
            &Expression::FieldAccess(field_access.clone()),
            schema,
        )?;
        
        // Check if type is numeric for sum/avg
        if matches!(function, "sum" | "avg") {
            if !matches!(field_type, FieldType::Integer | FieldType::Float) {
                return Err(OQLError::TypeError {
                    expected: "numeric type".to_string(),
                    actual: field_type.type_name().to_string(),
                });
            }
        }
        
        Ok(())
    }
}

impl Default for LogicalPlanner {
    fn default() -> Self {
        Self::new()
    }
}

impl LogicalPlan {
    /// Get the root node
    pub fn root(&self) -> &PlanNode {
        &self.root
    }
    
    /// Get the schema registry
    pub fn schema_registry(&self) -> Option<&SchemaRegistry> {
        self.schema_registry.as_ref()
    }
    
    /// Estimate the cardinality of the plan
    pub fn estimate_cardinality(&self) -> u64 {
        self.root.cardinality().unwrap_or(1000)
    }
    
    /// Estimate the cost of the plan
    pub fn estimate_cost(&self) -> f64 {
        self.root.cost().unwrap_or(100.0)
    }
}
