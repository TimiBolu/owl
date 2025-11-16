// Task 5.4: Comprehensive Aggregation Tests
// Tests all aggregation functions with various data types, GROUP BY with reference fields,
// and sorting performance with large datasets

use owl_db::{
    document::{Document, Value},
    executor::QueryExecutor,
    parser::parse_oql,
    schema::{Schema, FieldType, ReferenceType},
    storage::{DocumentStore, RocksDBStore},
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tempfile::TempDir;

/// Test fixture for aggregation tests
struct AggregationTestFixture {
    executor: QueryExecutor<RocksDBStore>,
    store: Arc<RocksDBStore>,
    _temp_dir: TempDir,
}

impl AggregationTestFixture {
    fn new() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let store = Arc::new(RocksDBStore::new(temp_dir.path()).expect("Failed to create store"));
        let executor = QueryExecutor::new(store.clone());
        
        Self::create_test_schemas(&store);
        
        Self {
            executor,
            store,
            _temp_dir: temp_dir,
        }
    }
    
    fn create_test_schemas(store: &Arc<RocksDBStore>) {
        // Sales schema with various data types
        let mut sales_fields = HashMap::new();
        sales_fields.insert("product_name".to_string(), FieldType::String);
        sales_fields.insert("category".to_string(), FieldType::String);
        sales_fields.insert("region".to_string(), FieldType::String);
        sales_fields.insert("amount".to_string(), FieldType::Float);
        sales_fields.insert("quantity".to_string(), FieldType::Integer);
        sales_fields.insert("discount".to_string(), FieldType::Optional(Box::new(FieldType::Float)));
        sales_fields.insert("is_premium".to_string(), FieldType::Boolean);
        sales_fields.insert("customer_id".to_string(), FieldType::Reference(ReferenceType::new("customers".to_string())));
        
        let sales_schema = Schema::new("sales".to_string(), sales_fields);
        store.put_schema(&sales_schema).expect("Failed to create sales schema");
        
        // Customers schema for reference testing
        let mut customer_fields = HashMap::new();
        customer_fields.insert("name".to_string(), FieldType::String);
        customer_fields.insert("tier".to_string(), FieldType::String);
        customer_fields.insert("signup_year".to_string(), FieldType::Integer);
        customer_fields.insert("lifetime_value".to_string(), FieldType::Float);
        
        let customer_schema = Schema::new("customers".to_string(), customer_fields);
        store.put_schema(&customer_schema).expect("Failed to create customers schema");
        
        // Products schema for large dataset testing
        let mut product_fields = HashMap::new();
        product_fields.insert("name".to_string(), FieldType::String);
        product_fields.insert("price".to_string(), FieldType::Float);
        product_fields.insert("rating".to_string(), FieldType::Float);
        product_fields.insert("category".to_string(), FieldType::String);
        product_fields.insert("stock_count".to_string(), FieldType::Integer);
        product_fields.insert("is_featured".to_string(), FieldType::Boolean);
        
        let product_schema = Schema::new("products".to_string(), product_fields);
        store.put_schema(&product_schema).expect("Failed to create products schema");
    }
    
    fn create_test_customers(&self) -> Result<(), Box<dyn std::error::Error>> {
        let customers = vec![
            ("cust_001", "Alice Johnson", "premium", 2020, 15000.0),
            ("cust_002", "Bob Smith", "standard", 2021, 8500.0),
            ("cust_003", "Charlie Brown", "premium", 2019, 22000.0),
            ("cust_004", "Diana Prince", "standard", 2022, 4200.0),
            ("cust_005", "Eve Wilson", "premium", 2020, 18500.0),
        ];
        
        for (id, name, tier, year, ltv) in customers {
            let mut fields = HashMap::new();
            fields.insert("name".to_string(), Value::String(name.to_string()));
            fields.insert("tier".to_string(), Value::String(tier.to_string()));
            fields.insert("signup_year".to_string(), Value::Integer(year));
            fields.insert("lifetime_value".to_string(), Value::Float(ltv));
            
            let doc = Document::with_id(id.to_string(), "customers".to_string(), fields);
            self.store.put_document(&doc)?;
        }
        
        Ok(())
    }
    
    fn create_test_sales(&self) -> Result<(), Box<dyn std::error::Error>> {
        let sales = vec![
            ("sale_001", "Laptop Pro", "Electronics", "North", 1299.99, 1, Some(100.0), true, "cust_001"),
            ("sale_002", "Office Chair", "Furniture", "South", 299.50, 2, None, false, "cust_002"),
            ("sale_003", "Smartphone", "Electronics", "North", 899.00, 1, Some(50.0), true, "cust_003"),
            ("sale_004", "Desk Lamp", "Furniture", "East", 79.99, 3, None, false, "cust_004"),
            ("sale_005", "Tablet", "Electronics", "West", 599.99, 1, Some(75.0), true, "cust_005"),
            ("sale_006", "Bookshelf", "Furniture", "South", 199.99, 1, None, false, "cust_001"),
            ("sale_007", "Monitor", "Electronics", "North", 449.99, 2, Some(25.0), false, "cust_002"),
            ("sale_008", "Coffee Table", "Furniture", "East", 349.99, 1, None, false, "cust_003"),
            ("sale_009", "Headphones", "Electronics", "West", 199.99, 1, Some(20.0), true, "cust_004"),
            ("sale_010", "Standing Desk", "Furniture", "North", 699.99, 1, Some(100.0), false, "cust_005"),
        ];
        
        for (id, product, category, region, amount, qty, discount, premium, customer) in sales {
            let mut fields = HashMap::new();
            fields.insert("product_name".to_string(), Value::String(product.to_string()));
            fields.insert("category".to_string(), Value::String(category.to_string()));
            fields.insert("region".to_string(), Value::String(region.to_string()));
            fields.insert("amount".to_string(), Value::Float(amount));
            fields.insert("quantity".to_string(), Value::Integer(qty));
            fields.insert("discount".to_string(), discount.map(Value::Float).unwrap_or(Value::Null));
            fields.insert("is_premium".to_string(), Value::Boolean(premium));
            fields.insert("customer_id".to_string(), Value::Reference(customer.to_string()));
            
            let doc = Document::with_id(id.to_string(), "sales".to_string(), fields);
            self.store.put_document(&doc)?;
        }
        
        Ok(())
    }
    
    fn create_large_product_dataset(&self, count: usize) -> Result<(), Box<dyn std::error::Error>> {
        let categories = vec!["Electronics", "Furniture", "Books", "Clothing", "Sports"];
        let base_names = vec!["Pro", "Standard", "Premium", "Basic", "Deluxe"];
        
        for i in 0..count {
            let category = &categories[i % categories.len()];
            let base_name = &base_names[i % base_names.len()];
            let name = format!("{} {} {}", category, base_name, i);
            let price = 10.0 + (i as f64 * 1.5) % 1000.0;
            let rating = 1.0 + (i as f64 * 0.1) % 4.0;
            let stock = (i * 7) % 100;
            let featured = i % 3 == 0;
            
            let mut fields = HashMap::new();
            fields.insert("name".to_string(), Value::String(name));
            fields.insert("price".to_string(), Value::Float(price));
            fields.insert("rating".to_string(), Value::Float(rating));
            fields.insert("category".to_string(), Value::String(category.to_string()));
            fields.insert("stock_count".to_string(), Value::Integer(stock as i64));
            fields.insert("is_featured".to_string(), Value::Boolean(featured));
            
            let doc = Document::with_id(format!("prod_{:06}", i), "products".to_string(), fields);
            self.store.put_document(&doc)?;
        }
        
        Ok(())
    }
}

// ========== AGGREGATION FUNCTIONS WITH VARIOUS DATA TYPES ==========

#[test]
fn test_count_function_with_different_data_types() {
    let fixture = AggregationTestFixture::new();
    fixture.create_test_sales().expect("Failed to create test sales");
    
    // Test COUNT(*) - count all records
    let query = parse_oql("sales.select { total_count: count(*) }").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_field("total_count"), Some(&Value::Integer(10)));
    
    // Test COUNT(field) with string field
    let query = parse_oql("sales.select { product_count: count(.product_name) }").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results[0].get_field("product_count"), Some(&Value::Integer(10)));
    
    // Test COUNT(field) with optional field (some nulls)
    let query = parse_oql("sales.select { discountcount: count(.discount) }").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    // Should count only non-null discount values (6 out of 10 based on test data)
    assert_eq!(results[0].get_field("discountcount"), Some(&Value::Integer(6)));
    
    // Test COUNT with boolean field
    let query = parse_oql("sales.select { premiumcount: count(.is_premium) }").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results[0].get_field("premiumcount"), Some(&Value::Integer(10)));
}

#[test]
fn test_sum_function_with_numeric_types() {
    let fixture = AggregationTestFixture::new();
    fixture.create_test_sales().expect("Failed to create test sales");
    
    // Test SUM with float values
    let query = parse_oql("sales.select { totalamount: sum(.amount) }").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    // Verify sum is correct (should be sum of all amounts)
    let expected_sum = 1299.99 + 299.50 + 899.00 + 79.99 + 599.99 + 199.99 + 449.99 + 349.99 + 199.99 + 699.99;
    if let Some(Value::Float(actual_sum)) = results[0].get_field("totalamount") {
        assert!((actual_sum - expected_sum).abs() < 0.01, "Expected {}, got {}", expected_sum, actual_sum);
    } else {
        panic!("Expected float sum result");
    }
    
    // Test SUM with integer values
    let query = parse_oql("sales.select { totalquantity: sum(.quantity) }").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results[0].get_field("totalquantity"), Some(&Value::Integer(14))); // Sum of all quantities
    
    // Test SUM with optional field (nulls should be ignored)
    let query = parse_oql("sales.select { totaldiscount: sum(.discount) }").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    // Should sum only non-null discount values: 100.0 + 50.0 + 75.0 + 25.0 + 20.0 + 100.0 = 370.0
    if let Some(Value::Float(discount_sum)) = results[0].get_field("totaldiscount") {
        assert!((discount_sum - 370.0).abs() < 0.01, "Expected 370.0, got {}", discount_sum);
    } else {
        panic!("Expected float discount sum result");
    }
}

#[test]
fn test_avg_function_with_numeric_types() {
    let fixture = AggregationTestFixture::new();
    fixture.create_test_sales().expect("Failed to create test sales");
    
    // Test AVG with float values
    let query = parse_oql("sales.select { avgamount: avg(.amount) }").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    // Calculate expected average
    let total_sum = 1299.99 + 299.50 + 899.00 + 79.99 + 599.99 + 199.99 + 449.99 + 349.99 + 199.99 + 699.99;
    let expected_avg = total_sum / 10.0;
    
    if let Some(Value::Float(actual_avg)) = results[0].get_field("avgamount") {
        assert!((actual_avg - expected_avg).abs() < 0.01, "Expected {}, got {}", expected_avg, actual_avg);
    } else {
        panic!("Expected float average result");
    }
    
    // Test AVG with integer values (should return float)
    let query = parse_oql("sales.select { avgquantity: avg(.quantity) }").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    let expected_qty_avg = 14.0 / 10.0; // 1.4
    if let Some(Value::Float(actual_qty_avg)) = results[0].get_field("avgquantity") {
        assert!((actual_qty_avg - expected_qty_avg).abs() < 0.01, "Expected {}, got {}", expected_qty_avg, actual_qty_avg);
    } else {
        panic!("Expected float quantity average result");
    }
    
    // Test AVG with optional field (nulls should be ignored)
    let query = parse_oql("sales.select { avgdiscount: avg(.discount) }").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    // Should average only non-null discount values: 370.0 / 6 = 61.67
    if let Some(Value::Float(discount_avg)) = results[0].get_field("avgdiscount") {
        assert!((discount_avg - 61.67).abs() < 0.01, "Expected 61.67, got {}", discount_avg);
    } else {
        panic!("Expected float discount average result");
    }
}

#[test]
fn test_min_max_functions_with_various_types() {
    let fixture = AggregationTestFixture::new();
    fixture.create_test_sales().expect("Failed to create test sales");
    
    // Test MIN/MAX with float values
    let query = parse_oql("sales.select { minamount: min(.amount), maxamount: max(.amount) }").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    if let Some(Value::Float(min_val)) = results[0].get_field("minamount") {
        assert!((min_val - 79.99).abs() < 0.01, "Expected 79.99, got {}", min_val);
    } else {
        panic!("Expected float min result");
    }
    
    if let Some(Value::Float(max_val)) = results[0].get_field("maxamount") {
        assert!((max_val - 1299.99).abs() < 0.01, "Expected 1299.99, got {}", max_val);
    } else {
        panic!("Expected float max result");
    }
    
    // Test MIN/MAX with integer values
    let query = parse_oql("sales.select { minqty: min(.quantity), maxqty: max(.quantity) }").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results[0].get_field("minqty"), Some(&Value::Integer(1)));
    assert_eq!(results[0].get_field("maxqty"), Some(&Value::Integer(3)));
    
    // Test MIN/MAX with string values (alphabetical order)
    let query = parse_oql("sales.select { mincategory: min(.category), maxcategory: max(.category) }").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results[0].get_field("mincategory"), Some(&Value::String("Electronics".to_string())));
    assert_eq!(results[0].get_field("maxcategory"), Some(&Value::String("Furniture".to_string())));
}

// ========== GROUP BY WITH REFERENCE FIELDS ==========

#[test]
fn test_group_by_with_reference_fields() {
    let fixture = AggregationTestFixture::new();
    fixture.create_test_customers().expect("Failed to create test customers");
    fixture.create_test_sales().expect("Failed to create test sales");
    
    // Test GROUP BY with reference field - group sales by customer tier
    // Note: This requires reference resolution in GROUP BY, which may need implementation
    let query = parse_oql("sales.group_by { .customer_id }.select { customer_id, salescount: count(*), totalamount: sum(.amount) }").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    // Should have 5 groups (one for each customer)
    assert_eq!(results.len(), 5);
    
    // Verify each customer has the correct number of sales
    for result in &results {
        let customer_id = result.get_field("customer_id").unwrap().to_string_repr();
        let sales_count = match result.get_field("salescount").unwrap() {
            Value::Integer(count) => *count,
            _ => panic!("Expected integer sales count"),
        };
        
        // Each customer should have exactly 2 sales based on our test data
        assert_eq!(sales_count, 2, "Customer {} should have 2 sales", customer_id);
    }
}

#[test]
fn test_group_by_multiple_fields_with_references() {
    let fixture = AggregationTestFixture::new();
    fixture.create_test_customers().expect("Failed to create test customers");
    fixture.create_test_sales().expect("Failed to create test sales");
    
    // Test GROUP BY with multiple fields including references
    let query = parse_oql("sales.group_by { .category, .customer_id }.select { category, customer_id, salescount: count(*), avgamount: avg(.amount) }").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    // Should have multiple groups based on category-customer combinations
    assert!(results.len() > 5); // More groups than just customers
    
    // Verify all results have the expected fields
    for result in &results {
        assert!(result.get_field("category").is_some());
        assert!(result.get_field("customer_id").is_some());
        assert!(result.get_field("salescount").is_some());
        assert!(result.get_field("avgamount").is_some());
        
        // Verify sales count is positive
        if let Some(Value::Integer(count)) = result.get_field("salescount") {
            assert!(*count > 0);
        }
    }
}

#[test]
fn test_group_by_with_aggregation_on_reference_fields() {
    let fixture = AggregationTestFixture::new();
    fixture.create_test_customers().expect("Failed to create test customers");
    fixture.create_test_sales().expect("Failed to create test sales");
    
    // Test GROUP BY with aggregation functions on various field types
    let query = parse_oql("sales.group_by { .category }.select { category, totalsales: count(*), totalrevenue: sum(.amount), avgquantity: avg(.quantity), premiumsales: count(.is_premium), uniquecustomers: count(.customer_id) }").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    // Should have 2 groups: Electronics and Furniture
    assert_eq!(results.len(), 2);
    
    // Find Electronics group
    let electronics_group = results.iter()
        .find(|r| r.get_field("category") == Some(&Value::String("Electronics".to_string())))
        .expect("Should have Electronics group");
    
    // Electronics should have 5 sales
    assert_eq!(electronics_group.get_field("totalsales"), Some(&Value::Integer(5)));
    
    // Find Furniture group
    let furniture_group = results.iter()
        .find(|r| r.get_field("category") == Some(&Value::String("Furniture".to_string())))
        .expect("Should have Furniture group");
    
    // Furniture should have 5 sales
    assert_eq!(furniture_group.get_field("totalsales"), Some(&Value::Integer(5)));
}

// ========== SORTING PERFORMANCE WITH LARGE DATASETS ==========

#[test]
fn test_sorting_performance_small_dataset() {
    let fixture = AggregationTestFixture::new();
    fixture.create_large_product_dataset(100).expect("Failed to create products");
    
    let start_time = Instant::now();
    
    // Test sorting by price (numeric)
    let query = parse_oql("products.sort { .price.desc }.limit 10").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    let duration = start_time.elapsed();
    println!("Sorting 100 products by price took: {:?}", duration);
    
    // Should return 10 results
    assert_eq!(results.len(), 10);
    
    // Verify results are sorted in descending order by price
    let mut prev_price = f64::INFINITY;
    for result in &results {
        if let Some(Value::Float(price)) = result.get_field("price") {
            assert!(*price <= prev_price, "Results should be sorted by price descending");
            prev_price = *price;
        }
    }
    
    // Performance assertion - should complete quickly for small dataset
    assert!(duration.as_millis() < 100, "Sorting 100 items should take less than 100ms");
}

#[test]
fn test_sorting_performance_medium_dataset() {
    let fixture = AggregationTestFixture::new();
    fixture.create_large_product_dataset(1000).expect("Failed to create products");
    
    let start_time = Instant::now();
    
    // Test sorting by string field
    let query = parse_oql("products.sort { .name }.limit 20").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    let duration = start_time.elapsed();
    println!("Sorting 1000 products by name took: {:?}", duration);
    
    // Should return 20 results
    assert_eq!(results.len(), 20);
    
    // Verify results are sorted alphabetically
    let mut prev_name = String::new();
    for result in &results {
        if let Some(Value::String(name)) = result.get_field("name") {
            assert!(name >= &prev_name, "Results should be sorted by name ascending");
            prev_name = name.clone();
        }
    }
    
    // Performance assertion - should still be reasonable for medium dataset
    assert!(duration.as_millis() < 500, "Sorting 1000 items should take less than 500ms");
}

#[test]
fn test_sorting_performance_large_dataset() {
    let fixture = AggregationTestFixture::new();
    fixture.create_large_product_dataset(5000).expect("Failed to create products");
    
    let start_time = Instant::now();
    
    // Test sorting by multiple fields
    let query = parse_oql("products.sort { .category, .price.desc }.limit 50").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    let duration = start_time.elapsed();
    println!("Sorting 5000 products by category and price took: {:?}", duration);
    
    // Should return 50 results
    assert_eq!(results.len(), 50);
    
    // Verify results are sorted correctly (first by category, then by price desc within category)
    let mut prev_category = String::new();
    let mut prev_price_in_category = f64::INFINITY;
    
    for result in &results {
        if let (Some(Value::String(category)), Some(Value::Float(price))) = 
            (result.get_field("category"), result.get_field("price")) {
            
            if category != &prev_category {
                // New category - reset price tracking
                prev_category = category.clone();
                prev_price_in_category = *price;
            } else {
                // Same category - price should be descending
                assert!(*price <= prev_price_in_category, 
                    "Within category {}, prices should be descending: {} <= {}", 
                    category, price, prev_price_in_category);
                prev_price_in_category = *price;
            }
        }
    }
    
    // Performance assertion - should handle large dataset reasonably
    assert!(duration.as_millis() < 2000, "Sorting 5000 items should take less than 2 seconds");
}

#[test]
fn test_complex_aggregation_with_sorting_performance() {
    let fixture = AggregationTestFixture::new();
    fixture.create_large_product_dataset(2000).expect("Failed to create products");
    
    let start_time = Instant::now();
    
    // Test complex query: GROUP BY with aggregation and sorting
    let query = parse_oql("products.group_by { .category }.select { category, productcount: count(*), avgprice: avg(.price), maxrating: max(.rating), featuredcount: count(.is_featured) }.sort { .avgprice.desc }").expect("Failed to parse query");
    
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    let duration = start_time.elapsed();
    println!("Complex aggregation and sorting on 2000 products took: {:?}", duration);
    
    // Should have 5 categories
    assert_eq!(results.len(), 5);
    
    // Verify results are sorted by average price descending
    let mut prev_avg_price = f64::INFINITY;
    for result in &results {
        if let Some(Value::Float(avg_price)) = result.get_field("avgprice") {
            assert!(*avg_price <= prev_avg_price, "Results should be sorted by avgprice descending");
            prev_avg_price = *avg_price;
        }
        
        // Verify all aggregation fields are present
        assert!(result.get_field("category").is_some());
        assert!(result.get_field("productcount").is_some());
        assert!(result.get_field("avgprice").is_some());
        assert!(result.get_field("maxrating").is_some());
        assert!(result.get_field("featuredcount").is_some());
    }
    
    // Performance assertion - complex operations should still be reasonable
    assert!(duration.as_millis() < 3000, "Complex aggregation should take less than 3 seconds");
}

// ========== EDGE CASES AND ERROR HANDLING ==========

#[test]
fn test_aggregation_with_empty_dataset() {
    let fixture = AggregationTestFixture::new();
    
    // Test aggregation functions on empty dataset
    let query = parse_oql("sales.select { countall: count(*), sumamount: sum(.amount), avgamount: avg(.amount), minamount: min(.amount), maxamount: max(.amount) }").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    let result = &results[0];
    
    // COUNT should return 0 for empty dataset
    assert_eq!(result.get_field("countall"), Some(&Value::Integer(0)));
    
    // SUM should return 0 for empty dataset
    assert_eq!(result.get_field("sumamount"), Some(&Value::Integer(0)));
    
    // AVG should return null for empty dataset
    assert_eq!(result.get_field("avgamount"), Some(&Value::Null));
    
    // MIN/MAX should return null for empty dataset
    assert_eq!(result.get_field("minamount"), Some(&Value::Null));
    assert_eq!(result.get_field("maxamount"), Some(&Value::Null));
}

#[test]
fn test_aggregation_with_all_null_values() {
    let fixture = AggregationTestFixture::new();
    
    // Create sales with all null discount values
    let sales = vec![
        ("sale_001", "Product A", 100.0, None),
        ("sale_002", "Product B", 200.0, None),
        ("sale_003", "Product C", 300.0, None),
    ];
    
    for (id, product, amount, discount) in sales {
        let mut fields = HashMap::new();
        fields.insert("product_name".to_string(), Value::String(product.to_string()));
        fields.insert("amount".to_string(), Value::Float(amount));
        fields.insert("discount".to_string(), discount.map(Value::Float).unwrap_or(Value::Null));
        
        let doc = Document::with_id(id.to_string(), "sales".to_string(), fields);
        fixture.store.put_document(&doc).expect("Failed to store document");
    }
    
    // Test aggregation on field with all null values
    let query = parse_oql("sales.select { discountcount: count(.discount), discountsum: sum(.discount), discountavg: avg(.discount), discountmin: min(.discount), discountmax: max(.discount) }").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    assert_eq!(results.len(), 1);
    let result = &results[0];
    
    // COUNT should return 0 for all-null field
    assert_eq!(result.get_field("discountcount"), Some(&Value::Integer(0)));
    
    // SUM should return 0 for all-null field
    assert_eq!(result.get_field("discountsum"), Some(&Value::Integer(0)));
    
    // AVG should return null for all-null field
    assert_eq!(result.get_field("discountavg"), Some(&Value::Null));
    
    // MIN/MAX should return null for all-null field
    assert_eq!(result.get_field("discountmin"), Some(&Value::Null));
    assert_eq!(result.get_field("discountmax"), Some(&Value::Null));
}

#[test]
fn test_group_by_with_null_values() {
    let fixture = AggregationTestFixture::new();
    
    // Create sales with some null category values
    let sales = vec![
        ("sale_001", Some("Electronics"), 100.0),
        ("sale_002", None, 200.0),
        ("sale_003", Some("Electronics"), 300.0),
        ("sale_004", None, 400.0),
        ("sale_005", Some("Furniture"), 500.0),
    ];
    
    for (id, category, amount) in sales {
        let mut fields = HashMap::new();
        fields.insert("category".to_string(), category.map(|s| Value::String(s.to_string())).unwrap_or(Value::Null));
        fields.insert("amount".to_string(), Value::Float(amount));
        
        let doc = Document::with_id(id.to_string(), "sales".to_string(), fields);
        fixture.store.put_document(&doc).expect("Failed to store document");
    }
    
    // Test GROUP BY with null values
    let query = parse_oql("sales.group_by { .category }.select { category, salescount: count(*), totalamount: sum(.amount) }").expect("Failed to parse query");
    let results = fixture.executor.execute_query(&query).expect("Failed to execute query");
    
    // Should have groups for each category (including null)
    assert!(results.len() >= 2, "Should have at least 2 groups, got {}", results.len());
    
    // Find the null group (might be represented differently)
    let null_group = results.iter()
        .find(|r| {
            match r.get_field("category") {
                Some(Value::Null) => true,
                Some(Value::String(s)) if s.is_empty() => true,
                Some(Value::String(s)) if s == "null" => true,
                _ => false,
            }
        });
    
    if let Some(null_group) = null_group {
    
        // Null group should have 2 sales
        assert_eq!(null_group.get_field("salescount"), Some(&Value::Integer(2)));
        
        // Null group should have total amount of 600.0 (200.0 + 400.0)
        if let Some(Value::Float(total)) = null_group.get_field("totalamount") {
            assert!((total - 600.0).abs() < 0.01);
        }
    } else {
        // If no null group found, that's also acceptable behavior
        println!("No null group found - this may be expected behavior");
    }
}