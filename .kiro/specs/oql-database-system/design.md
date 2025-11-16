# Design Document

## Overview

The OQL Database System is designed as a modern NoSQL document database that combines the expressiveness of SQL with the simplicity of document storage. The system features natural language query syntax, automatic reference resolution, and a progressive architecture that leverages proven technologies while focusing on core innovations.

The design emphasizes developer experience through intuitive syntax, predictable performance through simple document operations, and production readiness through battle-tested storage engines.

## Architecture

### High-Level Architecture

```
┌─────────────────┐    ┌─────────────────┐    ┌─────────────────┐
│   OQL Client    │    │   Web Console   │    │   CLI Tool      │
└─────────┬───────┘    └─────────┬───────┘    └─────────┬───────┘
          │                      │                      │
          └──────────────────────┼──────────────────────┘
                                 │
                    ┌─────────────▼─────────────┐
                    │    Arrow Flight Server    │
                    └─────────────┬─────────────┘
                                 │
                    ┌─────────────▼─────────────┐
                    │    OQL Query Engine       │
                    │  ┌─────────────────────┐  │
                    │  │   Parser & AST      │  │
                    │  │   Reference Resolver│  │
                    │  │   Query Executor    │  │
                    │  │   Function System   │  │
                    │  └─────────────────────┘  │
                    └─────────────┬─────────────┘
                                 │
                    ┌─────────────▼─────────────┐
                    │   Document Storage        │
                    │  ┌─────────────────────┐  │
                    │  │     RocksDB         │  │
                    │  │  (Document Store)   │  │
                    │  └─────────────────────┘  │
                    └───────────────────────────┘
```

### Core Design Principles

1. **Progressive Development** - Build incrementally with working systems at each phase
2. **Leverage Existing Tools** - Use proven technologies (RocksDB, Arrow Flight)
3. **Testable at Every Layer** - Each component independently testable
4. **Performance by Design** - Optimize for document operations and reference resolution
5. **Developer Experience First** - Natural language syntax and clear error messages

## Components and Interfaces

### 1. OQL Parser & AST

**Purpose**: Convert OQL text into a structured Abstract Syntax Tree for execution.

**Components**:
- **Tokenizer**: Converts OQL text into tokens using Rust's nom parser combinator
- **Parser**: Builds type-safe AST from tokens
- **AST Definitions**: Rust structs representing query structure
- **Error Reporter**: Provides human-friendly syntax error messages

**Key Interfaces**:
```rust
pub trait Parser {
    fn parse_query(input: &str) -> Result<QueryAST, ParseError>;
    fn parse_schema(input: &str) -> Result<SchemaAST, ParseError>;
}

pub struct QueryAST {
    pub from: TableReference,
    pub where_clause: Option<Expression>,
    pub select: SelectClause,
    pub order_by: Option<OrderByClause>,
    pub limit: Option<u64>,
}
```

### 2. Reference Resolution Engine

**Purpose**: Handle automatic navigation through document references (e.g., `.customer.account.secure_token`).

**Components**:
- **Reference Resolver**: Core logic for following reference chains
- **Batch Resolver**: Optimize multiple reference lookups
- **Cache Manager**: Cache frequently accessed documents
- **Circular Reference Detector**: Prevent infinite loops

**Key Interfaces**:
```rust
pub trait ReferenceResolver {
    fn resolve_path(&self, document: &Document, path: &[&str]) -> Result<Value, ResolveError>;
    fn batch_resolve(&self, refs: &[(DocumentId, &[&str])]) -> Vec<Result<Value, ResolveError>>;
}
```

### 3. Query Execution Engine

**Purpose**: Execute parsed queries against the document store with optimized performance.

**Components**:
- **Document Scanner**: Iterate through document collections
- **Filter Engine**: Apply WHERE conditions efficiently
- **Projection Engine**: Select and transform fields
- **Aggregation Engine**: Handle GROUP BY, COUNT, SUM operations
- **Sorting Engine**: Multi-field sorting with reference support

**Key Interfaces**:
```rust
pub trait QueryExecutor {
    fn execute_query(&self, query: QueryAST) -> Result<QueryResult, ExecutionError>;
    fn execute_stream(&self, query: QueryAST) -> Result<QueryStream, ExecutionError>;
}
```

### 4. Document Storage Layer

**Purpose**: Persist and retrieve documents using RocksDB as the storage engine.

**Components**:
- **Document Store**: Core document CRUD operations
- **Index Manager**: Secondary indexes for query optimization
- **Transaction Manager**: ACID transaction support
- **Schema Registry**: Store and validate document schemas

**Key Interfaces**:
```rust
pub trait DocumentStore {
    fn get_document(&self, id: &DocumentId) -> Result<Option<Document>, StorageError>;
    fn put_document(&self, id: &DocumentId, doc: &Document) -> Result<(), StorageError>;
    fn scan_documents(&self, schema: &str) -> Result<DocumentIterator, StorageError>;
}
```

### 5. Function System

**Purpose**: Support built-in and user-defined functions in queries.

**Components**:
- **Built-in Functions**: Math, string, date/time operations
- **Function Registry**: Register and resolve function calls
- **Type Checker**: Validate function arguments and return types
- **Function Cache**: Cache function results for performance

**Key Interfaces**:
```rust
pub trait FunctionRegistry {
    fn register_function(&mut self, name: &str, func: Box<dyn Function>);
    fn call_function(&self, name: &str, args: &[Value]) -> Result<Value, FunctionError>;
}
```

## Data Models

### Document Storage Model

Documents are stored as JSON-like structures with automatic reference resolution:

```rust
// Schema Definition
pub struct Schema {
    pub name: String,
    pub fields: HashMap<String, FieldType>,
}

pub enum FieldType {
    String,
    Integer,
    Float,
    Boolean,
    DateTime,
    Reference(String), // Reference to another schema
    List(Box<FieldType>),
    Optional(Box<FieldType>),
}

// Document Structure
pub struct Document {
    pub id: String,
    pub schema: String,
    pub fields: HashMap<String, Value>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

pub enum Value {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    DateTime(DateTime),
    Reference(String), // ID of referenced document
    List(Vec<Value>),
    Null,
}
```

### Storage Layout in RocksDB

```rust
// Column Families
CF: documents  → Document storage by schema and ID
CF: indexes    → Secondary indexes for queries
CF: schemas    → Schema definitions
CF: metadata   → System metadata

// Key Formats
documents: "{schema}#{id}" → Document JSON
indexes: "{schema}#{field}#{value}" → List of document IDs
schemas: "{schema_name}" → Schema definition JSON
```

### Reference Resolution Model

References are stored as simple string IDs and resolved automatically:

```rust
// Example Documents
Order {
    id: "ord_123",
    amount: 150.00,
    customer: "cust_456"  // Reference to customer document
}

Customer {
    id: "cust_456", 
    name: "Alice",
    account: "acc_789"   // Reference to account document
}

Account {
    id: "acc_789",
    balance: 5000.00,
    secure_token: "abc123xyz"
}

// Query with deep navigation
orders.where { .id == "ord_123" }
      .select { secure_token: .customer.account.secure_token }
```

## Error Handling

### Error Categories

1. **Parse Errors**: Syntax errors in OQL queries
2. **Type Errors**: Type mismatches in expressions
3. **Reference Errors**: Missing or invalid document references
4. **Storage Errors**: Database operation failures
5. **Function Errors**: Function call failures

### Error Handling Strategy

```rust
pub enum OQLError {
    ParseError(ParseError),
    TypeError(TypeError), 
    ReferenceError(ReferenceError),
    StorageError(StorageError),
    FunctionError(FunctionError),
}

impl OQLError {
    pub fn user_message(&self) -> String {
        match self {
            ParseError(e) => format!("Syntax error: {}", e.message),
            TypeError(e) => format!("Type error: Expected {}, got {}", e.expected, e.actual),
            ReferenceError(e) => format!("Reference error: Document {} not found", e.id),
            StorageError(e) => format!("Storage error: {}", e.message),
            FunctionError(e) => format!("Function error: {}", e.message),
        }
    }
}
```

### Error Recovery

- **Graceful Degradation**: Continue processing when possible
- **Partial Results**: Return available data with error annotations
- **Helpful Suggestions**: Provide correction suggestions for common errors
- **Context Preservation**: Include query context in error messages

## Testing Strategy

### Progressive Testing Approach

The testing strategy follows a progressive approach where each phase builds upon the previous one, ensuring that query capabilities expand incrementally and are thoroughly validated at each stage.

#### Phase 1: Foundation Testing (Basic Document Operations)
- **Document CRUD**: Insert, read, update, delete single documents
- **Simple Filtering**: Basic WHERE clauses with equality operators
- **Schema Validation**: Type checking and field validation
- **Storage Layer**: RocksDB integration and persistence

**Query Capabilities After Phase 1:**
```rust
// Basic document operations
customers.insert { id: "1", name: "Alice" }
customers.where { .id == "1" }
customers.where { .name == "Alice" }.update { name: "Alice Smith" }
```

#### Phase 2: Reference Resolution Testing (Navigation)
- **Single-Level References**: Navigate one level deep (.customer)
- **Multi-Level References**: Navigate multiple levels (.customer.account.token)
- **Reference Validation**: Ensure referenced documents exist
- **Circular Reference Detection**: Prevent infinite loops

**Query Capabilities After Phase 2:**
```rust
// Reference navigation
orders.where { .id == "ord_123" }.select { customer_name: .customer.name }
orders.select { secure_token: .customer.account.secure_token }
```

#### Phase 3: Advanced Querying Testing (Aggregation & Functions)
- **Aggregation Functions**: COUNT, SUM, AVG, MIN, MAX
- **Grouping Operations**: GROUP BY with reference fields
- **Sorting**: ORDER BY with multiple fields and references
- **Built-in Functions**: String, math, and date functions

**Query Capabilities After Phase 3:**
```rust
// Aggregation and grouping
customers.group_by { .tier }.select { tier: group_key, count: count(*) }
orders.select { total: sum(.items, .quantity * .price) }
customers.where { .created_at >= 30.days.ago }.count
```

#### Phase 4: Complex Business Logic Testing (Functions & Transactions)
- **User-Defined Functions**: Custom business logic functions
- **Transaction Support**: ACID properties and rollback
- **Conditional Logic**: IF/THEN/ELSE and pattern matching
- **Error Handling**: Graceful error recovery

**Query Capabilities After Phase 4:**
```rust
// Custom functions and transactions
fn customer_lifetime_value(customer: customers) -> Float {
    sum(customer.orders where .status == "completed", .total)
}

transaction ProcessOrder(customer_id: String, items: List<OrderItem>) {
    // Complex multi-step operations with rollback
}
```

#### Phase 5: Production Features Testing (Performance & Scalability)
- **Concurrent Access**: Multi-client operations
- **Performance Optimization**: Index usage and query planning
- **Large Dataset Handling**: Streaming and pagination
- **Real-time Operations**: Change streams and triggers

**Query Capabilities After Phase 5:**
```rust
// Production-ready features
stream OrderUpdates() -> Stream<Order> {
    orders.watch_changes().where { .status == "shipped" }
}

// Optimized queries with automatic indexing
customers.where { .email == "alice@example.com" } // Uses email index
```

### Testing Categories by Phase

#### Unit Testing (All Phases)
- **Parser Tests**: Syntax validation for each language construct
- **Function Tests**: Individual function correctness
- **Type System Tests**: Type inference and validation
- **Storage Tests**: Data persistence and retrieval

#### Integration Testing (Phases 2+)
- **Reference Resolution**: End-to-end navigation testing
- **Query Pipeline**: Complete query execution flow
- **Transaction Testing**: ACID compliance validation
- **Error Propagation**: Error handling across components

#### Performance Testing (Phases 3+)
- **Query Benchmarks**: Execution time measurements
- **Reference Resolution Performance**: Deep navigation efficiency
- **Concurrent Access**: Multi-client performance
- **Large Dataset**: Scalability with growing data

#### Compatibility Testing (All Phases)
- **SQL Equivalence**: Compare results with PostgreSQL
- **Document Operations**: Compare with MongoDB
- **Type Safety**: Verify compile-time error detection

## Performance Considerations

### Query Optimization

1. **Index Usage**: Automatic index selection for WHERE clauses
2. **Reference Batching**: Batch multiple reference lookups
3. **Result Streaming**: Stream large result sets
4. **Query Caching**: Cache frequently executed queries

### Storage Optimization

1. **Document Compression**: Use LZ4/Zstd compression in RocksDB
2. **Bloom Filters**: Fast existence checks for references
3. **Prefix Scans**: Efficient range queries on indexed fields
4. **Write Batching**: Batch multiple document writes

### Memory Management

1. **Reference Caching**: Cache frequently accessed documents
2. **Result Pagination**: Limit memory usage for large results
3. **Streaming Processing**: Process large datasets without loading entirely
4. **Connection Pooling**: Reuse database connections

## Security Considerations

### Authentication & Authorization

- **User Management**: Role-based access control
- **Query Permissions**: Fine-grained query access control
- **Schema Permissions**: Control schema modification access
- **Audit Logging**: Log all database operations

### Data Protection

- **Encryption at Rest**: Encrypt stored documents
- **Encryption in Transit**: TLS for all network communication
- **Input Validation**: Prevent injection attacks
- **Rate Limiting**: Prevent abuse and DoS attacks

This design provides a solid foundation for implementing the OQL Database System with clear separation of concerns, testable components, and a focus on developer experience while maintaining production-ready performance and reliability.