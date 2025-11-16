# OQL (Object Query Language) Syntax Reference

**Version**: 1.0  
**Last Updated**: November 2024

## Table of Contents

1. [Introduction](#introduction)
2. [Basic Concepts](#basic-concepts)
3. [Schema Definition](#schema-definition)
4. [Data Types](#data-types)
5. [CRUD Operations](#crud-operations)
6. [Field Access](#field-access)
7. [Filtering (WHERE)](#filtering-where)
8. [Sorting and Limiting](#sorting-and-limiting)
9. [Aggregation](#aggregation)
10. [Grouping](#grouping)
11. [Conditional Logic](#conditional-logic)
12. [Functions](#functions)
13. [String Operations](#string-operations)
14. [Array Operations](#array-operations)
15. [Date/Time Operations](#datetime-operations)
16. [Method Chaining](#method-chaining)
17. [Comments](#comments)
18. [Advanced Features](#advanced-features)

---

## Introduction

OQL (Object Query Language) is a NoSQL-style query language designed for the Owl database. It emphasizes:

- **Natural navigation** through object references (no JOINs)
- **Implicit null handling** for schema evolution
- **Direct object references** instead of foreign keys
- **Method chaining** for composable queries
- **Zero-copy pointer navigation** for performance

### Philosophy

OQL treats the database as a graph of objects with direct references, not as relational tables. Navigation is done through pointer dereferencing, making graph traversal essentially free (O(1)).

---

## Basic Concepts

### Collections

Collections are the primary data containers (similar to tables in SQL or collections in MongoDB):

```oql
users
orders
products
```

### Object References

Objects can reference other objects directly using `Ref<collection>`:

```oql
// Order references a user
orders.insert {
    id: "order_001",
    user_id: Ref<users>,  // Direct reference
    total: 299.99
}
```

### Navigation

Navigate through references using dot notation:

```oql
// Get user name from order
orders.select {
    order_id: .id,
    customer_name: .user_id.name  // Direct navigation
}
```

---

## Schema Definition

### Basic Schema

```oql
schema users {
    id: String,
    name: String,
    email: String,
    age: Integer,
    created_at: DateTime
}
```

### Optional Fields

Use `?` to mark fields as optional:

```oql
schema profiles {
    id: String,
    user_id: String,
    bio: String?,           // Optional
    avatar_url: String?,    // Optional
    website: String?        // Optional
}
```

### Nested Objects

Define nested object structures:

```oql
schema companies {
    id: String,
    name: String,
    address: {
        street: String,
        city: String,
        state: String,
        zip_code: String,
        country: String
    },
    contact: {
        email: String,
        phone: String?,
        website: String?
    }
}
```

### References

Define references to other collections:

```oql
schema orders {
    id: String,
    user_id: Ref<users>,    // Reference to users collection
    total: Float,
    status: String,
    created_at: DateTime
}
```

### Lists/Arrays

Define list fields:

```oql
schema orders {
    id: String,
    items: List<{
        product_id: String,
        quantity: Integer,
        price: Float
    }>,
    tags: List<String>
}
```

---

## Data Types

### Primitive Types

| Type | Description | Example |
|------|-------------|---------|
| `String` | Text data | `"Alice Smith"` |
| `Integer` | Whole numbers | `42`, `-10` |
| `Float` | Decimal numbers | `299.99`, `3.14` |
| `Bool` | Boolean | `true`, `false` |
| `DateTime` | ISO 8601 timestamps | `"2024-01-15T10:30:00Z"` |

### Complex Types

| Type | Description | Example |
|------|-------------|---------|
| `Ref<T>` | Reference to collection T | `Ref<users>` |
| `List<T>` | Array of type T | `List<String>` |
| `Object` | Nested object | `{ field: value }` |
| `T?` | Optional type T | `String?` |

---

## CRUD Operations

### Create (INSERT)

#### Single Document

```oql
users.insert {
    id: "user_001",
    name: "Alice Smith",
    email: "alice@example.com",
    age: 28,
    created_at: "2024-01-15T10:30:00Z"
}
```

#### With Nested Objects

```oql
companies.insert {
    id: "company_001",
    name: "Tech Corp",
    address: {
        street: "123 Main St",
        city: "San Francisco",
        state: "CA",
        zip_code: "94105"
    },
    contact: {
        email: "info@techcorp.com",
        phone: "+1-555-0123"
    }
}
```

#### With References

```oql
orders.insert {
    id: "order_001",
    user_id: "user_001",  // Reference to user
    total: 299.99,
    status: "pending",
    items: [
        {
            product_id: "prod_001",
            quantity: 2,
            price: 149.99
        }
    ]
}
```

#### Batch Insert

```oql
users.insert {
    id: "user_002",
    name: "Bob Jones",
    email: "bob@example.com",
    age: 35
}

users.insert {
    id: "user_003",
    name: "Carol White",
    email: "carol@example.com",
    age: 42
}
```

### Read (SELECT)

#### Select All

```oql
users

// Or explicitly
users.select { * }
```

#### Select Specific Fields

```oql
users.select {
    id,
    name,
    email
}
```

#### Select with Aliases

```oql
users.select {
    user_id: id,
    full_name: name,
    email_address: email,
    user_age: age
}
```

#### Select with Computed Fields

```oql
users.select {
    id,
    name,
    age,
    age_group: if .age >= 18 then "adult" else "minor"
}
```

### Update

#### Update Single Field

```oql
users.where { .id == "user_001" }.update {
    age: 29
}
```

#### Update Multiple Fields

```oql
users.where { .id == "user_002" }.update {
    age: 36,
    email: "bob.jones@example.com"
}
```

#### Update Nested Fields

```oql
companies.where { .id == "company_001" }.update {
    contact: {
        email: "contact@techcorp.com",
        phone: "+1-555-0199"
    }
}
```

#### Update with Expressions

```oql
users.where { .age < 30 }.update {
    age: .age + 1
}
```

### Delete

#### Delete by ID

```oql
users.where { .id == "user_003" }.delete
```

#### Delete with Condition

```oql
orders.where { .status == "cancelled" }.delete
```

#### Delete with Multiple Conditions

```oql
profiles.where { .bio == null && .avatar_url == null }.delete
```

---

## Field Access

### Simple Field Access

```oql
users.select {
    id,
    name,
    email,
    age
}
```

### Nested Field Access

```oql
companies.select {
    id,
    name,
    street: .address.street,
    city: .address.city,
    email: .contact.email
}
```

### Deep Nesting

```oql
users.select {
    id,
    theme: .profile.preferences.theme,
    email_notifications: .profile.preferences.notifications.email
}
```

### Reference Navigation

```oql
orders.select {
    id,
    user_name: .user_id.name,
    user_email: .user_id.email,
    user_city: .user_id.profile.address.city
}
```

### Null Coalescing

Use `||` for default values:

```oql
profiles.select {
    id,
    bio: .bio || "No bio provided",
    avatar: .avatar_url || "default.jpg",
    website: .website || "No website"
}
```

### Chained Null Coalescing

```oql
users.select {
    display_name: .profile.display_name || .profile.first_name || .name || "Anonymous"
}
```

### Array Access

```oql
orders.select {
    id,
    first_item: .items[0],
    first_product_id: .items[0].product_id
}
```

---

## Filtering (WHERE)

### Simple Conditions

#### Equality

```oql
users.where { .name == "Alice Smith" }
```

#### Inequality

```oql
users.where { .name != "Bob Jones" }
```

#### Numeric Comparisons

```oql
users.where { .age > 30 }
users.where { .age >= 25 }
users.where { .age < 40 }
users.where { .age <= 35 }
```

### Logical Operators

#### AND

```oql
users.where { .age > 25 && .age < 40 }
users.where { .name == "Alice Smith" && .age == 28 }
```

#### OR

```oql
users.where { .age < 25 || .age > 60 }
users.where { .name == "Alice Smith" || .name == "Bob Jones" }
```

#### NOT

```oql
users.where { not (.age < 18) }
users.where { not (.name == "Alice Smith") }
```

#### Complex Combinations

```oql
users.where { (.age > 25 && .age < 40) || .name == "Carol White" }
users.where { .age > 30 && (.name == "Bob Jones" || .email == "carol@example.com") }
```

### Nested Field Filtering

```oql
companies.where { .address.city == "San Francisco" }
companies.where { .address.state == "CA" && .contact.email != null }
```

### Null Checks

```oql
profiles.where { .bio == null }
profiles.where { .avatar_url != null }
profiles.where { .bio != null && .avatar_url != null }
```

### Reference Filtering

```oql
orders.where { .user_id == "user_001" }
orders.where { .user_id.name == "Alice Smith" }
orders.where { .user_id.age > 25 }
```

### String Operations in Filters

```oql
users.where { .email contains "@example.com" }
users.where { .name starts_with "A" }
users.where { .email ends_with ".com" }
```

### Range Filtering

```oql
users.where { .age >= 25 && .age <= 35 }
orders.where { .total >= 100.00 && .total <= 500.00 }
```

### Date Filtering

```oql
users.where { .created_at > "2024-01-01T00:00:00Z" }
orders.where { .created_at >= "2024-01-01T00:00:00Z" && .created_at < "2024-02-01T00:00:00Z" }
```

### IN Operator

```oql
users.where { .id in ["user1", "user2", "user3"] }
orders.where { .status in ["pending", "processing", "shipped"] }
```

---

## Sorting and Limiting

### Sort Ascending

```oql
users.sort { .age }
users.sort { .name }
```

### Sort Descending

```oql
users.sort { .age.desc }
users.sort { .created_at.desc }
```

### Multiple Sort Fields

```oql
users.sort { .age.desc, .name }
```

### Limit

```oql
users.limit(10)
users.limit(100)
```

### Offset

```oql
users.offset(20)
users.offset(100).limit(50)
```

### Combined

```oql
users
    .where { .age > 25 }
    .sort { .age.desc }
    .limit(10)
```

---

## Aggregation

### Count

```oql
// Count all
users.count()

// Count with filter
users.where { .age > 25 }.count()
```

### Sum

```oql
orders.sum(.total)
orders.where { .status == "completed" }.sum(.total)
```

### Average

```oql
orders.avg(.total)
users.avg(.age)
```

### Min/Max

```oql
orders.min(.total)
orders.max(.total)
users.min(.age)
users.max(.age)
```

### Multiple Aggregations

```oql
users.select {
    total_users: count(),
    avg_age: avg(.age),
    min_age: min(.age),
    max_age: max(.age)
}
```

---

## Grouping

### Simple Group By

```oql
orders.group_by { .status }
users.group_by { .age }
```

### Group by Nested Field

```oql
users.group_by { .preferences.theme }
orders.group_by { .shipping_address.state }
```

### Group with Aggregation

```oql
orders.group_by { .status }.select {
    status: .status,
    count: count(),
    total_revenue: sum(.total),
    avg_order_value: avg(.total)
}
```

### Having Clause

```oql
orders
    .group_by { .user_id }
    .having { count() > 5 }
    .select {
        user_id: .user_id,
        order_count: count(),
        total_spent: sum(.total)
    }
```

---

## Conditional Logic

### IF-ELSE Expressions

#### Simple Conditional

```oql
users.select {
    id,
    name,
    category: if .age >= 18 then "adult" else "minor"
}
```

#### IF-ELSE-IF Chains

```oql
users.select {
    id,
    age_group: if .age < 13 then "child"
               else if .age < 18 then "teen"
               else if .age < 65 then "adult"
               else "senior"
}
```

#### Nested Conditionals

```oql
users.select {
    id,
    access_level: if .age >= 18 then
                      if .preferences.notifications then "full_access"
                      else "limited_access"
                  else
                      if .age >= 13 then "teen_access"
                      else "child_access"
}
```

### Block Syntax

```oql
users.select {
    id,
    category: if .age >= 18 {
        "adult"
    } else {
        "minor"
    }
}
```

### Conditional with Null Handling

```oql
users.select {
    id,
    bio_status: if .profile.bio == null then "no_bio"
                else if .profile.bio.length() > 20 then "long_bio"
                else "short_bio"
}
```

### Conditional in WHERE

```oql
users.where {
    if .age >= 18 then .preferences.notifications == true
    else .preferences.theme == "light"
}
```

---

## Functions

### Built-in Functions

#### String Functions

```oql
.name.length()
.name.to_upper()
.name.to_lower()
.name.trim()
.email.split("@")
```

#### Array Functions

```oql
.items.count()
.tags.distinct()
.items.where { .price > 100 }
```

#### Date Functions

```oql
.created_at.days_since()
.updated_at.hours_since()
```

### User-Defined Functions

#### Simple Function

```oql
fn age_category(age: Integer) -> String {
    if age >= 18 {
        "adult"
    } else {
        "minor"
    }
}

users.select {
    id,
    name,
    category: age_category(.age)
}
```

#### Function with Multiple Parameters

```oql
fn calculate_discount(age: Integer, total: Float) -> Float {
    if age >= 65 {
        total * 0.2
    } else if age >= 18 {
        total * 0.1
    } else {
        total * 0.05
    }
}

orders.select {
    id,
    total,
    discount: calculate_discount(.user_id.age, .total)
}
```

#### Function Returning Objects

```oql
fn user_summary(user: Object) -> Object {
    {
        name: user.username,
        profile: user.profile.first_name + " " + user.profile.last_name,
        settings: {
            theme: user.preferences.theme,
            notifications: user.preferences.notifications
        }
    }
}
```

#### Recursive Functions

```oql
fn factorial(n: Integer) -> Integer {
    if n <= 1 {
        1
    } else {
        n * factorial(n - 1)
    }
}
```

---

## String Operations

### Concatenation

```oql
users.select {
    full_name: .first_name + " " + .last_name
}
```

### Contains

```oql
users.where { .email contains "@example.com" }
```

### Starts With

```oql
users.where { .name starts_with "A" }
```

### Ends With

```oql
users.where { .email ends_with ".com" }
```

### String Methods

```oql
users.select {
    upper_name: .name.to_upper(),
    lower_email: .email.to_lower(),
    trimmed: .bio.trim(),
    length: .name.length()
}
```

### Split

```oql
users.select {
    email_domain: .email.split("@")[1]
}
```

---

## Array Operations

### Count

```oql
orders.select {
    id,
    item_count: count(.items)
}
```

### Filter

```oql
orders.select {
    id,
    expensive_items: .items.where { .price > 100 }
}
```

### Map/Select

```oql
orders.select {
    id,
    product_names: .items.select { .product_id }
}
```

### Distinct

```oql
users.select {
    favorite_categories: .orders.items.product.category.distinct()
}
```

### Aggregations on Arrays

```oql
orders.select {
    id,
    total_items: count(.items),
    total_quantity: sum(.items.quantity),
    avg_price: avg(.items.price)
}
```

---

## Date/Time Operations

### Date Comparisons

```oql
users.where { .created_at > "2024-01-01T00:00:00Z" }
orders.where { .created_at >= "2024-01-15T00:00:00Z" }
```

### Date Ranges

```oql
orders.where {
    .created_at >= "2024-01-01T00:00:00Z" &&
    .created_at < "2024-02-01T00:00:00Z"
}
```

### Date Functions

```oql
users.select {
    id,
    account_age_days: .created_at.days_since(),
    account_status: if .created_at.days_since() > 365 then "veteran"
                    else if .created_at.days_since() > 30 then "regular"
                    else "new"
}
```

---

## Method Chaining

OQL supports fluent method chaining for composable queries:

```oql
users
    .where { .age > 25 }
    .where { .preferences.theme == "dark" }
    .sort { .age.desc }
    .limit(10)
    .select {
        id,
        name,
        age
    }
```

### Complex Chaining

```oql
orders
    .where { .user_id.age > 25 }
    .where { .total > 100 }
    .where { .status != "cancelled" }
    .sort { .created_at.desc }
    .limit(50)
    .select {
        order_id: .id,
        customer: .user_id.name,
        total: .total,
        status: .status
    }
```

---

## Comments

### Single-line Comments

```oql
// This is a single-line comment
users.where { .age > 25 }
```

### Multi-line Comments

```oql
/*
 * This is a multi-line comment
 * It can span multiple lines
 */
users.select { id, name }
```

### SQL-style Comments

```oql
-- This is also a comment
users.where { .age > 25 }
```

---

## Advanced Features

### Variables

```oql
let minAmount = 400.0
orders.where { .total > minAmount }
```

### Direct Object References

```oql
// Find product by ID and reference it
products.find("prod_001")

// Use in insert
orders.insert {
    id: "order_001",
    customer: users.find("user_001"),  // Direct reference
    items: [
        {
            product: products.find("prod_001"),  // Direct reference
            quantity: 2
        }
    ]
}
```

### Natural Navigation

```oql
// Navigate through multiple levels without JOINs
orders.select {
    order_id: .id,
    customer_name: .customer.profile.first_name + " " + .customer.profile.last_name,
    product_names: .items.product.name,
    product_categories: .items.product.category
}
```

### Implicit Null Handling

OQL gracefully handles missing fields across schema versions:

```oql
// Works on all schema versions
users.select {
    id,
    name,
    theme: .profile.preferences.theme || "default",
    notifications: .profile.preferences.notifications.email || true
}
```

### Complex Aggregations

```oql
{
    total_users: count(users),
    adult_users: count(users.where { .age >= 18 }),
    avg_age: avg(users.age),
    total_orders: count(orders),
    total_revenue: sum(orders.total)
}
```

### Cross-Collection Analysis

```oql
products.select {
    name: .name,
    price: .price,
    customer_reach: count(users.where { users.orders.items.product.id == products.id }),
    total_sold: sum(users.orders.items.where { .product.id == products.id }.quantity)
}
```

---

## Best Practices

### 1. Use Direct References

**Good:**
```oql
orders.select {
    customer_name: .customer.name
}
```

**Avoid:**
```oql
// Don't use foreign keys like SQL
orders.join(users).on(.user_id == users.id)
```

### 2. Leverage Null Coalescing

```oql
users.select {
    display_name: .profile.display_name || .name || "Anonymous"
}
```

### 3. Chain Methods for Readability

```oql
users
    .where { .age > 25 }
    .sort { .age.desc }
    .limit(10)
```

### 4. Use Meaningful Aliases

```oql
users.select {
    user_id: .id,
    full_name: .name,
    email_address: .email
}
```

### 5. Keep Conditionals Simple

```oql
// Good
users.select {
    category: if .age >= 18 then "adult" else "minor"
}

// Better for complex logic - use functions
fn categorize_user(age: Integer) -> String {
    if age < 13 then "child"
    else if age < 18 then "teen"
    else if age < 65 then "adult"
    else "senior"
}
```

---

## Performance Considerations

### 1. Navigation is Free

OQL uses pointer arithmetic for navigation, making it O(1):

```oql
// This is essentially free (0ns)
orders.select { .customer.profile.address.city }
```

### 2. Co-locate Related Objects

Use `allocNear()` in the storage layer to keep related objects on the same memory page for cache efficiency.

### 3. Filter Early

```oql
// Good - filter first
users.where { .age > 25 }.sort { .age }.limit(10)

// Less efficient - sort everything first
users.sort { .age }.where { .age > 25 }.limit(10)
```

### 4. Use Indexes

For frequently queried fields, create indexes:

```oql
index users on .email
index orders on .user_id
```

---

## Error Handling

### Graceful Null Handling

OQL handles missing fields gracefully:

```oql
// Returns null for missing fields, doesn't error
users.select {
    theme: .profile.preferences.theme
}
```

### Type Mismatches

Type errors are caught at query time:

```oql
// Error: Cannot compare string to integer
users.where { .name > 25 }
```

### Invalid References

```oql
// Error: Collection 'invalid_collection' does not exist
invalid_collection.select { * }
```

---

## Comparison with SQL

| SQL | OQL |
|-----|-----|
| `SELECT * FROM users` | `users` |
| `SELECT name, email FROM users` | `users.select { name, email }` |
| `WHERE age > 25` | `.where { .age > 25 }` |
| `ORDER BY age DESC` | `.sort { .age.desc }` |
| `LIMIT 10` | `.limit(10)` |
| `COUNT(*)` | `.count()` |
| `SUM(total)` | `.sum(.total)` |
| `JOIN orders ON users.id = orders.user_id` | `.user_id.name` (direct navigation) |
| `GROUP BY status` | `.group_by { .status }` |
| `HAVING COUNT(*) > 5` | `.having { count() > 5 }` |

---

## Complete Example

```oql
// Define schema
schema users {
    id: String,
    name: String,
    email: String,
    age: Integer,
    profile: {
        bio: String?,
        preferences: {
            theme: String?,
            notifications: Bool?
        }?
    }?,
    created_at: DateTime
}

schema orders {
    id: String,
    user_id: Ref<users>,
    total: Float,
    status: String,
    items: List<{
        product_id: String,
        quantity: Integer,
        price: Float
    }>,
    created_at: DateTime
}

// Insert data
users.insert {
    id: "user_001",
    name: "Alice Smith",
    email: "alice@example.com",
    age: 28,
    profile: {
        bio: "Software engineer",
        preferences: {
            theme: "dark",
            notifications: true
        }
    },
    created_at: "2024-01-15T10:30:00Z"
}

orders.insert {
    id: "order_001",
    user_id: "user_001",
    total: 299.99,
    status: "completed",
    items: [
        { product_id: "prod_001", quantity: 2, price: 149.99 }
    ],
    created_at: "2024-01-20T09:15:00Z"
}

// Complex query with navigation
orders
    .where { .total > 200 }
    .where { .user_id.age > 25 }
    .sort { .created_at.desc }
    .limit(10)
    .select {
        order_id: .id,
        customer_name: .user_id.name,
        customer_email: .user_id.email,
        customer_theme: .user_id.profile.preferences.theme || "default",
        total: .total,
        status: .status,
        item_count: count(.items),
        days_since_order: .created_at.days_since()
    }
```

---

## Conclusion

OQL is designed for natural, intuitive querying of object graphs. Its key innovations are:

1. **Direct navigation** through references (no JOINs)
2. **Implicit null handling** for schema evolution
3. **O(1) pointer navigation** for performance
4. **Method chaining** for composability
5. **NoSQL flexibility** with SQL-like familiarity

For more information, see the [OQL Architecture](./OQL_ARCHITECTURE.md) document.
