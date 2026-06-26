# Reference Navigation Fix - Deep Investigation

## Problem Statement
When using simplified reference syntax (`author: User` instead of `author: Ref<User>`), the system stores string IDs instead of ObjRef pointers, breaking O(1) navigation.

## Current Behavior
```oql
schema Post {
    author: User  // Parsed as String type, not Reference
}

Post.insert {
    author: "u1"  // Stored as Value::String("u1")
}

Post.select {
    author_name: .author.name  // Returns Null (can't navigate string)
}
```

## Expected Behavior
```oql
schema Post {
    author: User  // Should be recognized as Reference(User)
}

Post.insert {
    author: "u1"  // Should lookup User "u1" and store Value::Reference(objref)
}

Post.select {
    author_name: .author.name  // Should navigate ObjRef and return "Alice"
}
```

## Architecture Analysis

### Layer 1: Parser (`src/parser.rs`)
**Current State:**
- `SchemaField.field_type: String` - stores "User" as a string
- No type resolution happens during parsing
- Schema definitions are printed but not registered

**What Exists:**
```rust
pub struct SchemaField {
    pub name: String,
    pub field_type: String,  // Just a string!
    pub optional: bool,
}
```

**What's Needed:**
1. After parsing all schemas, resolve type names to actual types
2. Convert `field_type: "User"` to `FieldType::Reference(ReferenceType::new("User"))`
3. Register schemas with SchemaRegistry

### Layer 2: Executor (`src/executor.rs`)
**Current State:**
- `apply_insert_operation()` evaluates expressions to Values
- No schema-aware type conversion
- Validates against schema but doesn't use it for conversion

**What Exists:**
```rust
fn apply_insert_operation(&self, collection: &str, insert_block: &InsertBlock) -> OQLResult<Vec<Document>> {
    let mut fields = HashMap::new();
    for insert_field in &insert_block.fields {
        let value = self.evaluate_expression(&temp_doc, &insert_field.value)?;
        fields.insert(insert_field.name.clone(), value);  // Just stores the value as-is
    }
    // ...
}
```

**What's Needed:**
1. After evaluating expression, check schema for field type
2. If field type is Reference and value is String:
   - Look up the referenced document
   - Get its ObjRef using `store.get_objref()`
   - Convert to `Value::Reference(objref)`

### Layer 3: Storage (`src/storage.rs`, Zig bindings)
**Current State:** ✅ **ALREADY WORKS!**

**What Exists:**
```rust
// ZigStore already has these methods:
fn get_objref(&self, schema: &str, id: &DocumentId) -> OQLResult<Option<ObjRef>>
fn navigate_reference(&self, objref: &ObjRef) -> OQLResult<Option<Document>>
```

**Proof from tests:**
```rust
// From tests/test_o1_navigation.rs
let doc2_ref = zig_store.get_objref("test", &doc2.id).unwrap().unwrap();
doc1_fields.insert("next".to_string(), Value::Reference(doc2_ref));
let navigated_doc = zig_store.navigate_reference(&next_ref).unwrap();
// ✅ This works perfectly!
```

## Implementation Plan

### Phase 1: Schema Type Resolution
**File:** `src/parser.rs`

Add a function to convert SchemaDefinition to Schema:
```rust
pub fn schema_definition_to_schema(
    def: &SchemaDefinition,
    registry: &SchemaRegistry
) -> OQLResult<Schema> {
    let mut fields = HashMap::new();
    
    for field in &def.fields {
        let field_type = parse_field_type(&field.field_type, registry)?;
        let final_type = if field.optional {
            FieldType::Optional(Box::new(field_type))
        } else {
            field_type
        };
        fields.insert(field.name.clone(), final_type);
    }
    
    Ok(Schema::new(def.name.clone(), fields))
}

fn parse_field_type(type_str: &str, registry: &SchemaRegistry) -> OQLResult<FieldType> {
    match type_str {
        "String" => Ok(FieldType::String),
        "Integer" => Ok(FieldType::Integer),
        "Float" => Ok(FieldType::Float),
        "Boolean" => Ok(FieldType::Boolean),
        "DateTime" => Ok(FieldType::DateTime),
        // Check if it's a schema name (reference)
        schema_name if registry.exists(schema_name) => {
            Ok(FieldType::Reference(ReferenceType::new(schema_name.to_string())))
        }
        _ => Err(OQLError::ParseError {
            message: format!("Unknown type: {}", type_str)
        })
    }
}
```

### Phase 2: Schema Registration
**File:** `src/executor.rs` or `src/oql_runner.rs`

Update `execute_oql_file` to register schemas:
```rust
pub fn execute_oql_file<S: DocumentStore>(
    file: &OQLFile,
    executor: &QueryExecutor<S>,
) -> Result<OQLFileExecutionResult, Box<dyn std::error::Error>> {
    let registry = SchemaRegistry::new();
    
    // First pass: Register all schema names
    for statement in &file.statements {
        if let OQLFileStatement::Schema(schema_def) = statement {
            // Create empty schema just to register the name
            let empty_schema = Schema::new(schema_def.name.clone(), HashMap::new());
            registry.register(empty_schema)?;
        }
    }
    
    // Second pass: Convert and register full schemas with resolved types
    for statement in &file.statements {
        if let OQLFileStatement::Schema(schema_def) = statement {
            let schema = schema_definition_to_schema(schema_def, &registry)?;
            registry.register(schema)?;
            executor.store().put_schema(&schema)?;
        }
    }
    
    // Third pass: Execute queries
    // ...
}
```

### Phase 3: Reference Resolution During Insert
**File:** `src/executor.rs`

Update `apply_insert_operation`:
```rust
fn apply_insert_operation(&self, collection: &str, insert_block: &InsertBlock) -> OQLResult<Vec<Document>> {
    let mut fields = HashMap::new();
    let temp_doc = Document::new(collection.to_string(), HashMap::new());
    
    // Get schema if it exists
    let schema = self.store.get_schema(collection)?;
    
    for insert_field in &insert_block.fields {
        let mut value = self.evaluate_expression(&temp_doc, &insert_field.value)?;
        
        // Check if this field should be a reference
        if let Some(ref schema) = schema {
            if let Some(field_type) = schema.get_field_type(&insert_field.name) {
                value = self.convert_to_schema_type(value, field_type)?;
            }
        }
        
        fields.insert(insert_field.name.clone(), value);
    }
    
    // ... rest of the function
}

fn convert_to_schema_type(&self, value: Value, field_type: &FieldType) -> OQLResult<Value> {
    match (value, field_type) {
        // If we have a string but schema expects a reference
        (Value::String(id), FieldType::Reference(ref_type)) => {
            // Look up the referenced document
            let target_schema = &ref_type.target_schema;
            
            // Get the ObjRef for this document
            if let Some(objref) = self.store.get_objref(target_schema, &id)? {
                Ok(Value::Reference(objref))
            } else {
                Err(OQLError::ReferenceError {
                    message: format!("Referenced document not found: {} in {}", id, target_schema)
                })
            }
        }
        // Pass through if types already match
        (v, _) => Ok(v)
    }
}
```

### Phase 4: Add get_objref to DocumentStore Trait
**File:** `src/storage.rs`

The trait needs to expose `get_objref`:
```rust
pub trait DocumentStore: Send + Sync {
    // ... existing methods ...
    
    /// Get the ObjRef for a document (for creating references)
    fn get_objref(&self, schema: &str, id: &DocumentId) -> OQLResult<Option<ObjRef>> {
        // Default implementation returns None
        Ok(None)
    }
}
```

ZigStore already implements this, just needs to be in the trait.

## Testing Strategy

### Test 1: Simple Reference
```oql
schema User {
    id: String,
    name: String
}

schema Post {
    id: String,
    author: User
}

User.insert { id: "u1", name: "Alice" }
Post.insert { id: "p1", author: "u1" }

// Should return "Alice", not Null
Post.select { author_name: .author.name }
```

### Test 2: Multi-hop Navigation
```oql
schema User { id: String, name: String }
schema Post { id: String, author: User }
schema Comment { id: String, post: Post }

User.insert { id: "u1", name: "Alice" }
Post.insert { id: "p1", author: "u1" }
Comment.insert { id: "c1", post: "p1" }

// Should navigate: Comment -> Post -> User
Comment.select { author: .post.author.name }
```

### Test 3: Optional References
```oql
schema Post {
    id: String,
    author: User?  // Optional reference
}

Post.insert { id: "p1" }  // No author
Post.select { author_name: .author.name }  // Should return Null gracefully
```

## Performance Considerations

1. **Schema Resolution**: O(1) hash lookup per field
2. **ObjRef Lookup**: O(1) hash lookup in Zig arena
3. **Reference Navigation**: O(1) pointer dereference
4. **Overall**: Still maintains O(1) navigation performance

## Backward Compatibility

- Existing `Ref<User>` syntax still works
- New `User` syntax is more natural
- Both compile to the same `FieldType::Reference`
- No breaking changes to storage layer

## Summary

**What Works:**
- ✅ Storage layer (Zig arena, ObjRef, navigation)
- ✅ Value::Reference type
- ✅ navigate_reference() method

**What Needs Implementation:**
- ❌ Schema type resolution (String -> FieldType)
- ❌ Schema registration in executor
- ❌ Reference conversion during insert
- ❌ get_objref in DocumentStore trait

**Estimated Effort:**
- Phase 1: 2-3 hours (schema resolution)
- Phase 2: 1 hour (registration)
- Phase 3: 2-3 hours (insert conversion)
- Phase 4: 30 minutes (trait update)
- Testing: 1-2 hours

**Total: ~7-10 hours of focused development**

The foundation is solid. We just need to connect the dots between the parser and the storage layer.
