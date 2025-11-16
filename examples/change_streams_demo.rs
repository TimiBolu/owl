// Change Streams and Triggers Demo
// Demonstrates real-time document change tracking and automatic triggers

use owl_db::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

fn main() -> OQLResult<()> {
    println!("=== OQL Change Streams and Triggers Demo ===\n");

    // Create a temporary database
    let temp_dir = TempDir::new().unwrap();
    let store = Arc::new(storage::RocksDBStore::new(temp_dir.path())?);
    let change_store = ChangeTrackingStore::new(store);

    // Create schemas
    setup_schemas(&change_store)?;

    // Demo 1: Basic change tracking
    demo_change_tracking(&change_store)?;

    // Demo 2: Change stream subscriptions
    demo_change_streams(&change_store)?;

    // Demo 3: Triggers
    demo_triggers(&change_store)?;

    // Demo 4: Real-time query result updates
    demo_real_time_updates(&change_store)?;

    println!("\n=== Change Streams and Triggers Demo Complete ===");
    Ok(())
}

fn setup_schemas(store: &ChangeTrackingStore<storage::RocksDBStore>) -> OQLResult<()> {
    println!("Setting up schemas...");

    // Users schema
    let mut user_fields = HashMap::new();
    user_fields.insert("name".to_string(), FieldType::String);
    user_fields.insert("email".to_string(), FieldType::String);
    user_fields.insert("status".to_string(), FieldType::String);
    user_fields.insert("tier".to_string(), FieldType::String);
    let users_schema = Schema::new("users".to_string(), user_fields);
    store.put_schema(&users_schema)?;

    // Orders schema
    let mut order_fields = HashMap::new();
    order_fields.insert("customer_id".to_string(), FieldType::Reference(
        crate::schema::ReferenceType {
            target_schema: "users".to_string(),
            required: true,
            on_delete: crate::schema::CascadeBehavior::Restrict,
        }
    ));
    order_fields.insert("amount".to_string(), FieldType::Float);
    order_fields.insert("status".to_string(), FieldType::String);
    order_fields.insert("items".to_string(), FieldType::Integer);
    let orders_schema = Schema::new("orders".to_string(), order_fields);
    store.put_schema(&orders_schema)?;

    // Notifications schema
    let mut notification_fields = HashMap::new();
    notification_fields.insert("user_id".to_string(), FieldType::String);
    notification_fields.insert("message".to_string(), FieldType::String);
    notification_fields.insert("type".to_string(), FieldType::String);
    let notifications_schema = Schema::new("notifications".to_string(), notification_fields);
    store.put_schema(&notifications_schema)?;

    println!("✓ Schemas created\n");
    Ok(())
}

fn demo_change_tracking(store: &ChangeTrackingStore<storage::RocksDBStore>) -> OQLResult<()> {
    println!("--- Demo 1: Basic Change Tracking ---");

    // Create a user
    let mut user_fields = HashMap::new();
    user_fields.insert("name".to_string(), Value::String("Alice Johnson".to_string()));
    user_fields.insert("email".to_string(), Value::String("alice@example.com".to_string()));
    user_fields.insert("status".to_string(), Value::String("active".to_string()));
    user_fields.insert("tier".to_string(), Value::String("premium".to_string()));
    let user = Document::with_id("user_1".to_string(), "users".to_string(), user_fields);

    println!("Creating user: {}", user.get_field("name").unwrap().to_string_repr());
    store.put_document(&user)?;

    // Update the user
    let mut updated_fields = user.fields.clone();
    updated_fields.insert("email".to_string(), Value::String("alice.johnson@example.com".to_string()));
    updated_fields.insert("tier".to_string(), Value::String("enterprise".to_string()));
    let updated_user = Document::with_id("user_1".to_string(), "users".to_string(), updated_fields);

    println!("Updating user email and tier...");
    store.put_document(&updated_user)?;

    // Delete the user
    println!("Deleting user...");
    store.delete_document("users", &"user_1".to_string())?;

    // Show change history
    let changes = store.change_manager().get_change_history(None)?;
    println!("\nChange History:");
    for (i, change) in changes.iter().enumerate() {
        println!("  {}. {} - {} at {}", 
            i + 1, 
            change.document_id, 
            format!("{:?}", change.change_type),
            change.timestamp.format("%H:%M:%S")
        );
        if !change.changed_fields.is_empty() {
            println!("     Changed fields: {:?}", change.changed_fields);
        }
    }

    println!("✓ Change tracking demo complete\n");
    Ok(())
}

fn demo_change_streams(store: &ChangeTrackingStore<storage::RocksDBStore>) -> OQLResult<()> {
    println!("--- Demo 2: Change Stream Subscriptions ---");

    // Subscribe to user changes
    let user_filter = ChangeFilter::new()
        .schemas(vec!["users".to_string()])
        .change_types(vec![ChangeType::Insert, ChangeType::Update]);
    let user_stream = store.change_manager().subscribe(user_filter)?;

    // Subscribe to order changes
    let order_filter = ChangeFilter::new()
        .schemas(vec!["orders".to_string()]);
    let order_stream = store.change_manager().subscribe(order_filter)?;

    // Start a thread to monitor changes
    let user_stream_id = user_stream.id.clone();
    let order_stream_id = order_stream.id.clone();
    
    thread::spawn(move || {
        println!("📡 Monitoring user changes...");
        while let Some(change) = user_stream.next_change() {
            println!("  🔔 User change detected: {} - {:?}", 
                change.document_id, change.change_type);
            if let Some(doc) = change.document() {
                if let Some(name) = doc.get_field("name") {
                    println!("     User: {}", name.to_string_repr());
                }
            }
        }
    });

    thread::spawn(move || {
        println!("📡 Monitoring order changes...");
        while let Some(change) = order_stream.next_change() {
            println!("  🔔 Order change detected: {} - {:?}", 
                change.document_id, change.change_type);
            if let Some(doc) = change.document() {
                if let Some(amount) = doc.get_field("amount") {
                    println!("     Amount: ${}", amount.to_string_repr());
                }
            }
        }
    });

    // Give threads time to start
    thread::sleep(Duration::from_millis(100));

    // Create some test data
    println!("Creating test users and orders...");

    // Create users
    for i in 1..=3 {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String(format!("User {}", i)));
        fields.insert("email".to_string(), Value::String(format!("user{}@example.com", i)));
        fields.insert("status".to_string(), Value::String("active".to_string()));
        fields.insert("tier".to_string(), Value::String("standard".to_string()));
        let user = Document::with_id(format!("user_{}", i), "users".to_string(), fields);
        store.put_document(&user)?;
        thread::sleep(Duration::from_millis(50));
    }

    // Create orders
    for i in 1..=2 {
        let mut fields = HashMap::new();
        fields.insert("customer_id".to_string(), Value::Reference(format!("user_{}", i)));
        fields.insert("amount".to_string(), Value::Float(100.0 * i as f64));
        fields.insert("status".to_string(), Value::String("pending".to_string()));
        fields.insert("items".to_string(), Value::Integer(i as i64));
        let order = Document::with_id(format!("order_{}", i), "orders".to_string(), fields);
        store.put_document(&order)?;
        thread::sleep(Duration::from_millis(50));
    }

    // Update a user
    let mut updated_fields = HashMap::new();
    updated_fields.insert("name".to_string(), Value::String("Updated User 1".to_string()));
    updated_fields.insert("email".to_string(), Value::String("user1@example.com".to_string()));
    updated_fields.insert("status".to_string(), Value::String("active".to_string()));
    updated_fields.insert("tier".to_string(), Value::String("premium".to_string()));
    let updated_user = Document::with_id("user_1".to_string(), "users".to_string(), updated_fields);
    store.put_document(&updated_user)?;

    // Give time for changes to be processed
    thread::sleep(Duration::from_millis(200));

    // Unsubscribe
    store.change_manager().unsubscribe(&user_stream_id)?;
    store.change_manager().unsubscribe(&order_stream_id)?;

    println!("✓ Change streams demo complete\n");
    Ok(())
}

fn demo_triggers(store: &ChangeTrackingStore<storage::RocksDBStore>) -> OQLResult<()> {
    println!("--- Demo 3: Triggers ---");

    // Register trigger for new user notifications
    let new_user_trigger = Trigger::new(
        "new_user_notification".to_string(),
        "users".to_string(),
        vec![ChangeType::Insert],
        TriggerAction::Notification {
            message: "Welcome to our platform!".to_string(),
            recipients: vec!["admin@example.com".to_string()],
        },
    );
    let trigger_id1 = store.change_manager().register_trigger(new_user_trigger)?;
    println!("✓ Registered new user notification trigger");

    // Register trigger for order status changes
    let order_status_trigger = Trigger::new(
        "order_status_change".to_string(),
        "orders".to_string(),
        vec![ChangeType::Update],
        TriggerAction::Function {
            name: "update_inventory".to_string(),
            args: vec![],
        },
    );
    let trigger_id2 = store.change_manager().register_trigger(order_status_trigger)?;
    println!("✓ Registered order status change trigger");

    // Register trigger for premium user upgrades
    let premium_upgrade_trigger = Trigger::new(
        "premium_upgrade".to_string(),
        "users".to_string(),
        vec![ChangeType::Update],
        TriggerAction::Notification {
            message: "Thank you for upgrading to premium!".to_string(),
            recipients: vec!["sales@example.com".to_string()],
        },
    );
    let trigger_id3 = store.change_manager().register_trigger(premium_upgrade_trigger)?;
    println!("✓ Registered premium upgrade trigger");

    // List all triggers
    let triggers = store.change_manager().list_triggers()?;
    println!("\nRegistered triggers:");
    for trigger in &triggers {
        println!("  - {} ({}): {:?} on {}", 
            trigger.name, 
            if trigger.enabled { "enabled" } else { "disabled" },
            trigger.change_types,
            trigger.schema
        );
    }

    // Test triggers by creating data
    println!("\nTesting triggers...");

    // Create a new user (should trigger new_user_notification)
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String("Bob Wilson".to_string()));
    fields.insert("email".to_string(), Value::String("bob@example.com".to_string()));
    fields.insert("status".to_string(), Value::String("active".to_string()));
    fields.insert("tier".to_string(), Value::String("standard".to_string()));
    let user = Document::with_id("user_trigger_test".to_string(), "users".to_string(), fields);
    store.put_document(&user)?;

    // Update user to premium (should trigger premium_upgrade)
    let mut updated_fields = user.fields.clone();
    updated_fields.insert("tier".to_string(), Value::String("premium".to_string()));
    let updated_user = Document::with_id("user_trigger_test".to_string(), "users".to_string(), updated_fields);
    store.put_document(&updated_user)?;

    // Create and update an order (should trigger order_status_change)
    let mut order_fields = HashMap::new();
    order_fields.insert("customer_id".to_string(), Value::Reference("user_trigger_test".to_string()));
    order_fields.insert("amount".to_string(), Value::Float(250.0));
    order_fields.insert("status".to_string(), Value::String("pending".to_string()));
    order_fields.insert("items".to_string(), Value::Integer(3));
    let order = Document::with_id("order_trigger_test".to_string(), "orders".to_string(), order_fields);
    store.put_document(&order)?;

    // Update order status
    let mut updated_order_fields = order.fields.clone();
    updated_order_fields.insert("status".to_string(), Value::String("shipped".to_string()));
    let updated_order = Document::with_id("order_trigger_test".to_string(), "orders".to_string(), updated_order_fields);
    store.put_document(&updated_order)?;

    // Give time for triggers to execute
    thread::sleep(Duration::from_millis(100));

    // Disable a trigger
    store.change_manager().set_trigger_enabled(&trigger_id2, false)?;
    println!("\n✓ Disabled order status change trigger");

    // Clean up triggers
    store.change_manager().remove_trigger(&trigger_id1)?;
    store.change_manager().remove_trigger(&trigger_id2)?;
    store.change_manager().remove_trigger(&trigger_id3)?;

    println!("✓ Triggers demo complete\n");
    Ok(())
}

fn demo_real_time_updates(store: &ChangeTrackingStore<storage::RocksDBStore>) -> OQLResult<()> {
    println!("--- Demo 4: Real-time Query Result Updates ---");

    // This demonstrates how change streams can be used to maintain real-time query results
    // In a full implementation, this would integrate with the query executor

    // Subscribe to changes that affect a specific query
    let filter = ChangeFilter::new()
        .schemas(vec!["users".to_string()])
        .field_filter("tier".to_string(), Value::String("premium".to_string()));
    
    let stream = store.change_manager().subscribe(filter)?;
    let stream_id = stream.id.clone();

    // Simulate a real-time dashboard monitoring premium users
    thread::spawn(move || {
        println!("📊 Real-time dashboard: Monitoring premium users...");
        while let Some(change) = stream.next_change() {
            if let Some(doc) = change.document() {
                if let (Some(name), Some(_tier)) = (doc.get_field("name"), doc.get_field("tier")) {
                    match change.change_type {
                        ChangeType::Insert => {
                            println!("  📈 New premium user: {}", name.to_string_repr());
                        }
                        ChangeType::Update => {
                            if change.field_changed("tier") {
                                println!("  ⬆️  User upgraded to premium: {}", name.to_string_repr());
                            } else {
                                println!("  📝 Premium user updated: {}", name.to_string_repr());
                            }
                        }
                        ChangeType::Delete => {
                            println!("  📉 Premium user removed: {}", name.to_string_repr());
                        }
                    }
                }
            }
        }
    });

    // Give thread time to start
    thread::sleep(Duration::from_millis(50));

    // Create some test scenarios
    println!("Simulating user activity...");

    // Create a standard user
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String("Charlie Brown".to_string()));
    fields.insert("email".to_string(), Value::String("charlie@example.com".to_string()));
    fields.insert("status".to_string(), Value::String("active".to_string()));
    fields.insert("tier".to_string(), Value::String("standard".to_string()));
    let user = Document::with_id("user_realtime_1".to_string(), "users".to_string(), fields);
    store.put_document(&user)?;
    thread::sleep(Duration::from_millis(100));

    // Upgrade to premium (should trigger dashboard update)
    let mut premium_fields = user.fields.clone();
    premium_fields.insert("tier".to_string(), Value::String("premium".to_string()));
    let premium_user = Document::with_id("user_realtime_1".to_string(), "users".to_string(), premium_fields);
    store.put_document(&premium_user)?;
    thread::sleep(Duration::from_millis(100));

    // Create another premium user directly
    let mut fields2 = HashMap::new();
    fields2.insert("name".to_string(), Value::String("Diana Prince".to_string()));
    fields2.insert("email".to_string(), Value::String("diana@example.com".to_string()));
    fields2.insert("status".to_string(), Value::String("active".to_string()));
    fields2.insert("tier".to_string(), Value::String("premium".to_string()));
    let user2 = Document::with_id("user_realtime_2".to_string(), "users".to_string(), fields2);
    store.put_document(&user2)?;
    thread::sleep(Duration::from_millis(100));

    // Update premium user info
    let mut updated_fields = user2.fields.clone();
    updated_fields.insert("email".to_string(), Value::String("diana.prince@example.com".to_string()));
    let updated_user2 = Document::with_id("user_realtime_2".to_string(), "users".to_string(), updated_fields);
    store.put_document(&updated_user2)?;
    thread::sleep(Duration::from_millis(100));

    // Downgrade a user (won't trigger since filter is for premium tier)
    let mut downgraded_fields = premium_user.fields.clone();
    downgraded_fields.insert("tier".to_string(), Value::String("standard".to_string()));
    let downgraded_user = Document::with_id("user_realtime_1".to_string(), "users".to_string(), downgraded_fields);
    store.put_document(&downgraded_user)?;
    thread::sleep(Duration::from_millis(100));

    // Clean up
    store.change_manager().unsubscribe(&stream_id)?;

    println!("✓ Real-time updates demo complete\n");
    Ok(())
}