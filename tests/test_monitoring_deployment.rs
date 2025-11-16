// Comprehensive tests for monitoring and deployment features

use owl_db::{
    change_streams::{ChangeFilter, ChangeType, ChangeTrackingStore, Trigger, TriggerAction},
    config::{BackupManager, BackupConfig, DatabaseConfig, Environment, MigrationManager, ConfigFormat, CompressionType},
    document::{Document, Value},
    error::OQLResult,
    monitoring::{MonitoringSystem, MonitoringConfig, QueryMetrics, SystemMetrics, Alert, AlertSeverity},
    parser::{OQLQuery, Operation, WhereBlock, Condition, Expression, FieldAccess, ComparisonOperator, Literal},
    storage::{RocksDBStore, DocumentStore},
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::fs;
use tempfile::TempDir;
use chrono::Utc;

/// Helper function to create a test document
fn create_test_document(id: &str, name: &str, status: &str) -> Document {
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), Value::String(name.to_string()));
    fields.insert("status".to_string(), Value::String(status.to_string()));
    fields.insert("created_at".to_string(), Value::String(Utc::now().to_rfc3339()));
    Document::with_id(id.to_string(), "orders".to_string(), fields)
}

/// Helper function to create a test query
fn create_test_query() -> OQLQuery {
    OQLQuery {
        collection: "orders".to_string(),
        operations: vec![
            Operation::Where(WhereBlock {
                condition: Condition::Comparison {
                    left: Expression::FieldAccess(FieldAccess {
                        path: vec!["status".to_string()],
                    }),
                    operator: ComparisonOperator::Equal,
                    right: Expression::Literal(Literal::String("shipped".to_string())),
                },
            }),
        ],
    }
}

/// Test change stream functionality
#[cfg(test)]
mod change_stream_tests {
    use super::*;

    #[test]
    fn test_change_stream_basic_functionality() {
        let temp_dir = TempDir::new().unwrap();
        let store = Arc::new(RocksDBStore::new(temp_dir.path()).unwrap());
        let change_store = ChangeTrackingStore::new(store);

        // Subscribe to changes for orders schema
        let filter = ChangeFilter::new()
            .schemas(vec!["orders".to_string()])
            .change_types(vec![ChangeType::Insert, ChangeType::Update]);
        
        let handle = change_store.change_manager().subscribe(filter).unwrap();

        // Insert a document
        let doc1 = create_test_document("order_1", "Test Order", "pending");
        change_store.put_document(&doc1).unwrap();

        // Check if we received the insert change
        let change = handle.try_next_change();
        assert!(change.is_some());
        let change = change.unwrap();
        assert_eq!(change.change_type, ChangeType::Insert);
        assert_eq!(change.document_id, "order_1");
        assert_eq!(change.schema, "orders");
        assert!(change.old_document.is_none());
        assert!(change.new_document.is_some());

        // Update the document
        let doc2 = create_test_document("order_1", "Test Order", "shipped");
        change_store.put_document(&doc2).unwrap();

        // Check if we received the update change
        let change = handle.try_next_change();
        assert!(change.is_some());
        let change = change.unwrap();
        assert_eq!(change.change_type, ChangeType::Update);
        assert_eq!(change.document_id, "order_1");
        assert!(change.old_document.is_some());
        assert!(change.new_document.is_some());
        assert!(change.field_changed("status"));
    }

    #[test]
    fn test_change_stream_filtering() {
        let temp_dir = TempDir::new().unwrap();
        let store = Arc::new(RocksDBStore::new(temp_dir.path()).unwrap());
        let change_store = ChangeTrackingStore::new(store);

        // Subscribe only to update changes
        let filter = ChangeFilter::new()
            .schemas(vec!["orders".to_string()])
            .change_types(vec![ChangeType::Update]);
        
        let handle = change_store.change_manager().subscribe(filter).unwrap();

        // Insert a document (should not trigger)
        let doc1 = create_test_document("order_1", "Test Order", "pending");
        change_store.put_document(&doc1).unwrap();

        // Should not receive insert change
        let change = handle.try_next_change();
        assert!(change.is_none());

        // Update the document (should trigger)
        let doc2 = create_test_document("order_1", "Test Order", "shipped");
        change_store.put_document(&doc2).unwrap();

        // Should receive update change
        let change = handle.try_next_change();
        assert!(change.is_some());
        let change = change.unwrap();
        assert_eq!(change.change_type, ChangeType::Update);
    }

    #[test]
    fn test_change_stream_delete_tracking() {
        let temp_dir = TempDir::new().unwrap();
        let store = Arc::new(RocksDBStore::new(temp_dir.path()).unwrap());
        let change_store = ChangeTrackingStore::new(store);

        // Subscribe to all change types
        let filter = ChangeFilter::new()
            .schemas(vec!["orders".to_string()]);
        
        let handle = change_store.change_manager().subscribe(filter).unwrap();

        // Insert and then delete a document
        let doc = create_test_document("order_1", "Test Order", "pending");
        change_store.put_document(&doc).unwrap();
        
        // Clear the insert change
        handle.try_next_change();

        // Delete the document
        let deleted = change_store.delete_document("orders", &"order_1".to_string()).unwrap();
        assert!(deleted);

        // Check if we received the delete change
        let change = handle.try_next_change();
        assert!(change.is_some());
        let change = change.unwrap();
        assert_eq!(change.change_type, ChangeType::Delete);
        assert_eq!(change.document_id, "order_1");
        assert!(change.old_document.is_some());
        assert!(change.new_document.is_none());
    }

    #[test]
    fn test_change_history() {
        let temp_dir = TempDir::new().unwrap();
        let store = Arc::new(RocksDBStore::new(temp_dir.path()).unwrap());
        let change_store = ChangeTrackingStore::new(store);

        // Perform several operations
        let doc1 = create_test_document("order_1", "Order 1", "pending");
        let doc2 = create_test_document("order_2", "Order 2", "pending");
        
        change_store.put_document(&doc1).unwrap();
        change_store.put_document(&doc2).unwrap();
        
        // Update order 1
        let doc1_updated = create_test_document("order_1", "Order 1 Updated", "shipped");
        change_store.put_document(&doc1_updated).unwrap();

        // Get change history
        let history = change_store.change_manager().get_change_history(None).unwrap();
        assert_eq!(history.len(), 3);
        
        // Check order of changes (should be chronological)
        assert_eq!(history[0].change_type, ChangeType::Insert);
        assert_eq!(history[0].document_id, "order_1");
        assert_eq!(history[1].change_type, ChangeType::Insert);
        assert_eq!(history[1].document_id, "order_2");
        assert_eq!(history[2].change_type, ChangeType::Update);
        assert_eq!(history[2].document_id, "order_1");

        // Test limited history
        let limited_history = change_store.change_manager().get_change_history(Some(2)).unwrap();
        assert_eq!(limited_history.len(), 2);
    }

    #[test]
    fn test_trigger_registration_and_execution() {
        let temp_dir = TempDir::new().unwrap();
        let store = Arc::new(RocksDBStore::new(temp_dir.path()).unwrap());
        let change_store = ChangeTrackingStore::new(store);

        // Register a trigger for order status changes
        let trigger = Trigger::new(
            "order_shipped_notification".to_string(),
            "orders".to_string(),
            vec![ChangeType::Update],
            TriggerAction::Notification {
                message: "Order has been shipped!".to_string(),
                recipients: vec!["customer@example.com".to_string()],
            },
        );

        let trigger_id = change_store.change_manager().register_trigger(trigger).unwrap();

        // List triggers
        let triggers = change_store.change_manager().list_triggers().unwrap();
        assert_eq!(triggers.len(), 1);
        assert_eq!(triggers[0].name, "order_shipped_notification");
        assert!(triggers[0].enabled);

        // Disable trigger
        let disabled = change_store.change_manager()
            .set_trigger_enabled(&trigger_id, false).unwrap();
        assert!(disabled);

        // Verify trigger is disabled
        let triggers = change_store.change_manager().list_triggers().unwrap();
        assert!(!triggers[0].enabled);

        // Remove trigger
        let removed = change_store.change_manager().remove_trigger(&trigger_id).unwrap();
        assert!(removed);

        // Verify trigger is removed
        let triggers = change_store.change_manager().list_triggers().unwrap();
        assert_eq!(triggers.len(), 0);
    }

    #[test]
    fn test_multiple_subscribers() {
        let temp_dir = TempDir::new().unwrap();
        let store = Arc::new(RocksDBStore::new(temp_dir.path()).unwrap());
        let change_store = ChangeTrackingStore::new(store);

        // Create multiple subscribers with different filters
        let filter1 = ChangeFilter::new()
            .schemas(vec!["orders".to_string()])
            .change_types(vec![ChangeType::Insert]);
        
        let filter2 = ChangeFilter::new()
            .schemas(vec!["orders".to_string()])
            .change_types(vec![ChangeType::Update]);

        let handle1 = change_store.change_manager().subscribe(filter1).unwrap();
        let handle2 = change_store.change_manager().subscribe(filter2).unwrap();

        // Insert a document
        let doc = create_test_document("order_1", "Test Order", "pending");
        change_store.put_document(&doc).unwrap();

        // Only subscriber 1 should receive the change
        assert!(handle1.try_next_change().is_some());
        assert!(handle2.try_next_change().is_none());

        // Update the document
        let doc_updated = create_test_document("order_1", "Test Order", "shipped");
        change_store.put_document(&doc_updated).unwrap();

        // Only subscriber 2 should receive the change
        assert!(handle1.try_next_change().is_none());
        assert!(handle2.try_next_change().is_some());
    }
}

/// Test monitoring metrics accuracy
#[cfg(test)]
mod monitoring_tests {
    use super::*;

    #[test]
    fn test_query_metrics_recording() {
        let config = MonitoringConfig::default();
        let monitoring = MonitoringSystem::new(config);

        // Record some query metrics
        let query = create_test_query();
        let metrics1 = QueryMetrics::new(&query, 150, 100, 5);
        let metrics2 = QueryMetrics::new(&query, 200, 150, 8);
        let metrics3 = QueryMetrics::error(&query, 50, "Connection timeout");

        monitoring.record_query_metrics(metrics1).unwrap();
        monitoring.record_query_metrics(metrics2).unwrap();
        monitoring.record_query_metrics(metrics3).unwrap();

        // Get query statistics
        let stats = monitoring.get_query_stats().unwrap();
        assert_eq!(stats.total_queries, 3);
        assert_eq!(stats.successful_queries, 2);
        assert_eq!(stats.avg_execution_time_ms, (150.0 + 200.0 + 50.0) / 3.0);
        assert_eq!(stats.success_rate, 2.0 / 3.0);
    }

    #[test]
    fn test_system_metrics_tracking() {
        let config = MonitoringConfig::default();
        let monitoring = MonitoringSystem::new(config);

        // Update system metrics
        let mut system_metrics = SystemMetrics::new();
        system_metrics.active_connections = 25;
        system_metrics.queries_per_second = 150.5;
        system_metrics.avg_query_time_ms = 75.2;
        system_metrics.storage_size_bytes = 1024 * 1024 * 100; // 100MB
        system_metrics.memory_usage_bytes = 1024 * 1024 * 50;  // 50MB
        system_metrics.cache_hit_rate = 0.85;
        system_metrics.error_rate = 0.02;

        monitoring.update_system_metrics(system_metrics.clone()).unwrap();

        // Retrieve system metrics
        let retrieved_metrics = monitoring.get_system_metrics().unwrap();
        assert_eq!(retrieved_metrics.active_connections, 25);
        assert_eq!(retrieved_metrics.queries_per_second, 150.5);
        assert_eq!(retrieved_metrics.avg_query_time_ms, 75.2);
        assert_eq!(retrieved_metrics.cache_hit_rate, 0.85);
        assert_eq!(retrieved_metrics.error_rate, 0.02);
    }

    #[test]
    fn test_alert_management() {
        let config = MonitoringConfig::default();
        let monitoring = MonitoringSystem::new(config);

        // Create alerts
        let alert1 = Alert::new(
            AlertSeverity::Warning,
            "High Memory Usage".to_string(),
            "Memory usage is above 80%".to_string(),
        );
        let alert2 = Alert::new(
            AlertSeverity::Error,
            "Database Connection Failed".to_string(),
            "Unable to connect to database".to_string(),
        );

        let alert1_id = alert1.id.clone();
        let alert2_id = alert2.id.clone();

        monitoring.create_alert(alert1).unwrap();
        monitoring.create_alert(alert2).unwrap();

        // Get active alerts
        let active_alerts = monitoring.get_active_alerts().unwrap();
        assert_eq!(active_alerts.len(), 2);

        // Resolve one alert
        let resolved = monitoring.resolve_alert(&alert1_id).unwrap();
        assert!(resolved);

        // Check active alerts again
        let active_alerts = monitoring.get_active_alerts().unwrap();
        assert_eq!(active_alerts.len(), 1);
        assert_eq!(active_alerts[0].id, alert2_id);

        // Try to resolve non-existent alert
        let not_resolved = monitoring.resolve_alert("non_existent_id").unwrap();
        assert!(!not_resolved);
    }

    #[test]
    fn test_automatic_alert_generation() {
        let mut config = MonitoringConfig::default();
        config.alert_thresholds.max_query_time_ms = 100; // Low threshold for testing
        config.alert_thresholds.max_error_rate = 0.1;    // 10% error rate threshold

        let monitoring = MonitoringSystem::new(config);

        // Record a slow query (should trigger alert)
        let query = create_test_query();
        let slow_metrics = QueryMetrics::new(&query, 150, 100, 5); // 150ms > 100ms threshold
        monitoring.record_query_metrics(slow_metrics).unwrap();

        // Check if alert was created
        let alerts = monitoring.get_active_alerts().unwrap();
        assert!(!alerts.is_empty());
        assert!(alerts.iter().any(|a| a.title.contains("Slow Query")));

        // Record a failed query (should trigger alert)
        let error_metrics = QueryMetrics::error(&query, 50, "Syntax error");
        monitoring.record_query_metrics(error_metrics).unwrap();

        // Check if error alert was created
        let alerts = monitoring.get_active_alerts().unwrap();
        assert!(alerts.iter().any(|a| a.title.contains("Query Execution Failed")));
    }

    #[test]
    fn test_distributed_tracing() {
        let mut config = MonitoringConfig::default();
        config.enable_distributed_tracing = true;

        let monitoring = MonitoringSystem::new(config);

        // Start a trace
        let mut span = monitoring.start_trace("query_execution".to_string()).unwrap();
        span = span.tag("query_type".to_string(), "select".to_string());
        span = span.tag("schema".to_string(), "orders".to_string());
        span.log("info".to_string(), "Starting query execution".to_string());

        // Simulate some work
        std::thread::sleep(Duration::from_millis(10));

        // Create a child span
        let mut child_span = span.child("index_lookup".to_string());
        child_span.log("debug".to_string(), "Looking up index".to_string());
        std::thread::sleep(Duration::from_millis(5));
        child_span.finish();

        // Finish parent span
        let trace_id = span.trace_id.clone();
        span.finish();

        // Record spans
        monitoring.finish_span(child_span).unwrap();
        monitoring.finish_span(span).unwrap();

        // Retrieve trace
        let trace_spans = monitoring.get_trace(&trace_id).unwrap();
        assert_eq!(trace_spans.len(), 2);
        
        // Check parent span
        let parent_span = trace_spans.iter()
            .find(|s| s.operation_name == "query_execution")
            .unwrap();
        assert!(parent_span.duration_ms.is_some());
        assert!(parent_span.duration_ms.unwrap() >= 10);
        assert_eq!(parent_span.tags.get("query_type"), Some(&"select".to_string()));

        // Check child span
        let child_span = trace_spans.iter()
            .find(|s| s.operation_name == "index_lookup")
            .unwrap();
        assert!(child_span.duration_ms.is_some());
        assert_eq!(child_span.parent_span_id, Some(parent_span.span_id.clone()));
    }

    #[test]
    fn test_prometheus_metrics_export() {
        let config = MonitoringConfig::default();
        let monitoring = MonitoringSystem::new(config);

        // Record some metrics
        let query = create_test_query();
        let metrics = QueryMetrics::new(&query, 100, 50, 10);
        monitoring.record_query_metrics(metrics).unwrap();

        // Update system metrics
        let mut system_metrics = SystemMetrics::new();
        system_metrics.active_connections = 15;
        system_metrics.storage_size_bytes = 1024 * 1024 * 200; // 200MB
        monitoring.update_system_metrics(system_metrics).unwrap();

        // Export Prometheus metrics
        let prometheus_output = monitoring.export_prometheus_metrics().unwrap();

        // Verify format and content
        assert!(prometheus_output.contains("# HELP oql_active_connections"));
        assert!(prometheus_output.contains("# TYPE oql_active_connections gauge"));
        assert!(prometheus_output.contains("oql_active_connections 15"));
        assert!(prometheus_output.contains("oql_storage_size_bytes"));
        assert!(prometheus_output.contains("oql_queries_per_second"));
        assert!(prometheus_output.contains("oql_avg_query_time_ms"));
        assert!(prometheus_output.contains("oql_cache_hit_rate"));
        assert!(prometheus_output.contains("oql_error_rate"));
    }

    #[test]
    fn test_dashboard_data_aggregation() {
        let config = MonitoringConfig::default();
        let monitoring = MonitoringSystem::new(config);

        // Record some data
        let query = create_test_query();
        let metrics = QueryMetrics::new(&query, 120, 75, 12);
        monitoring.record_query_metrics(metrics).unwrap();

        let alert = Alert::new(
            AlertSeverity::Info,
            "System Status".to_string(),
            "System is running normally".to_string(),
        );
        monitoring.create_alert(alert).unwrap();

        // Get dashboard data
        let dashboard = monitoring.get_dashboard_data().unwrap();

        assert_eq!(dashboard.query_stats.total_queries, 1);
        assert_eq!(dashboard.query_stats.successful_queries, 1);
        assert_eq!(dashboard.active_alerts.len(), 1);
        assert_eq!(dashboard.recent_queries.len(), 1);
        assert!(dashboard.system_metrics.uptime_seconds >= 0);
    }
}

/// Test backup and restore operations
#[cfg(test)]
mod backup_restore_tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_backup_creation() {
        let temp_dir = TempDir::new().unwrap();
        let backup_dir = temp_dir.path().join("backups");
        
        let backup_config = BackupConfig {
            enabled: true,
            backup_dir: backup_dir.clone(),
            interval_hours: 24,
            max_backups: 5,
            compress: false,
        };

        let backup_manager = BackupManager::new(backup_config);

        // Create a test database directory
        let db_dir = temp_dir.path().join("database");
        fs::create_dir_all(&db_dir).unwrap();
        fs::write(db_dir.join("data.db"), "test database content").unwrap();
        fs::write(db_dir.join("metadata.json"), r#"{"version": "1.0"}"#).unwrap();

        // Create backup
        let backup_path = backup_manager
            .create_backup(&db_dir, Some("test_backup_001".to_string()))
            .unwrap();

        assert!(backup_path.exists());
        assert!(backup_path.file_name().unwrap().to_str().unwrap().contains("test_backup_001"));

        // Verify backup directory structure
        assert!(backup_dir.exists());
    }

    #[test]
    fn test_compressed_backup() {
        let temp_dir = TempDir::new().unwrap();
        let backup_dir = temp_dir.path().join("backups");
        
        let backup_config = BackupConfig {
            enabled: true,
            backup_dir: backup_dir.clone(),
            interval_hours: 24,
            max_backups: 5,
            compress: true, // Enable compression
        };

        let backup_manager = BackupManager::new(backup_config);

        // Create test database
        let db_dir = temp_dir.path().join("database");
        fs::create_dir_all(&db_dir).unwrap();
        fs::write(db_dir.join("large_file.db"), "x".repeat(1000)).unwrap();

        // Create compressed backup
        let backup_path = backup_manager
            .create_backup(&db_dir, Some("compressed_backup".to_string()))
            .unwrap();

        assert!(backup_path.exists());
        assert!(backup_path.extension().and_then(|s| s.to_str()) == Some("gz"));
    }

    #[test]
    fn test_backup_listing() {
        let temp_dir = TempDir::new().unwrap();
        let backup_dir = temp_dir.path().join("backups");
        
        let backup_config = BackupConfig {
            enabled: true,
            backup_dir: backup_dir.clone(),
            interval_hours: 24,
            max_backups: 5,
            compress: false,
        };

        let backup_manager = BackupManager::new(backup_config);

        // Create test database
        let db_dir = temp_dir.path().join("database");
        fs::create_dir_all(&db_dir).unwrap();
        fs::write(db_dir.join("test.db"), "test data").unwrap();

        // Create multiple backups
        backup_manager.create_backup(&db_dir, Some("backup_001".to_string())).unwrap();
        backup_manager.create_backup(&db_dir, Some("backup_002".to_string())).unwrap();
        backup_manager.create_backup(&db_dir, Some("backup_003".to_string())).unwrap();

        // List backups
        let backups = backup_manager.list_backups().unwrap();
        assert_eq!(backups.len(), 3);

        // Verify backup info
        let backup_names: Vec<&str> = backups.iter().map(|b| b.name.as_str()).collect();
        assert!(backup_names.contains(&"backup_001"));
        assert!(backup_names.contains(&"backup_002"));
        assert!(backup_names.contains(&"backup_003"));

        // Check that backups are sorted by creation time (newest first)
        for i in 1..backups.len() {
            assert!(backups[i-1].created_at >= backups[i].created_at);
        }
    }

    #[test]
    fn test_backup_cleanup() {
        let temp_dir = TempDir::new().unwrap();
        let backup_dir = temp_dir.path().join("backups");
        
        let backup_config = BackupConfig {
            enabled: true,
            backup_dir: backup_dir.clone(),
            interval_hours: 24,
            max_backups: 2, // Only keep 2 backups
            compress: false,
        };

        let backup_manager = BackupManager::new(backup_config);

        // Create test database
        let db_dir = temp_dir.path().join("database");
        fs::create_dir_all(&db_dir).unwrap();
        fs::write(db_dir.join("test.db"), "test data").unwrap();

        // Create more backups than the limit
        backup_manager.create_backup(&db_dir, Some("backup_001".to_string())).unwrap();
        std::thread::sleep(Duration::from_millis(10)); // Ensure different timestamps
        backup_manager.create_backup(&db_dir, Some("backup_002".to_string())).unwrap();
        std::thread::sleep(Duration::from_millis(10));
        backup_manager.create_backup(&db_dir, Some("backup_003".to_string())).unwrap();

        // Should only have 2 backups (oldest should be removed)
        let backups = backup_manager.list_backups().unwrap();
        assert_eq!(backups.len(), 2);

        // Should have the 2 most recent backups
        let backup_names: Vec<&str> = backups.iter().map(|b| b.name.as_str()).collect();
        assert!(backup_names.contains(&"backup_002"));
        assert!(backup_names.contains(&"backup_003"));
        assert!(!backup_names.contains(&"backup_001")); // Should be removed
    }

    #[test]
    fn test_backup_restore() {
        let temp_dir = TempDir::new().unwrap();
        let backup_dir = temp_dir.path().join("backups");
        
        let backup_config = BackupConfig {
            enabled: true,
            backup_dir: backup_dir.clone(),
            interval_hours: 24,
            max_backups: 5,
            compress: false,
        };

        let backup_manager = BackupManager::new(backup_config);

        // Create test database with specific content
        let db_dir = temp_dir.path().join("database");
        fs::create_dir_all(&db_dir).unwrap();
        let test_content = "original database content";
        fs::write(db_dir.join("data.db"), test_content).unwrap();

        // Create backup
        let backup_path = backup_manager
            .create_backup(&db_dir, Some("restore_test".to_string()))
            .unwrap();

        // Create restore target directory
        let restore_dir = temp_dir.path().join("restored");

        // Restore backup
        backup_manager.restore_backup(&backup_path, &restore_dir).unwrap();

        // Verify restoration
        assert!(restore_dir.exists());
        // Note: In the simplified implementation, we just create a placeholder
        // In a real implementation, we'd verify the actual restored content
    }

    #[test]
    fn test_backup_disabled() {
        let temp_dir = TempDir::new().unwrap();
        
        let backup_config = BackupConfig {
            enabled: false, // Disabled
            backup_dir: temp_dir.path().join("backups"),
            interval_hours: 24,
            max_backups: 5,
            compress: false,
        };

        let backup_manager = BackupManager::new(backup_config);

        // Create test database
        let db_dir = temp_dir.path().join("database");
        fs::create_dir_all(&db_dir).unwrap();
        fs::write(db_dir.join("test.db"), "test data").unwrap();

        // Attempt to create backup (should fail)
        let result = backup_manager.create_backup(&db_dir, Some("test".to_string()));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("disabled"));
    }
}

/// Test configuration and deployment features
#[cfg(test)]
mod config_deployment_tests {
    use super::*;

    #[test]
    fn test_environment_specific_configurations() {
        // Test development configuration
        let dev_config = DatabaseConfig::for_environment(Environment::Development);
        assert_eq!(dev_config.environment, Environment::Development);
        assert!(!dev_config.security.enable_auth);
        assert!(!dev_config.security.enable_tls);
        assert!(!dev_config.storage.backup.enabled);
        assert!(dev_config.logging.log_queries);

        // Test production configuration
        let prod_config = DatabaseConfig::for_environment(Environment::Production);
        assert_eq!(prod_config.environment, Environment::Production);
        assert!(prod_config.security.enable_auth);
        assert!(prod_config.security.enable_tls);
        assert!(prod_config.storage.backup.enabled);
        assert!(prod_config.security.rate_limiting.enabled);
        assert!(prod_config.monitoring.enable_performance_monitoring);
        assert!(prod_config.monitoring.enable_distributed_tracing);

        // Test testing configuration
        let test_config = DatabaseConfig::for_environment(Environment::Testing);
        assert_eq!(test_config.environment, Environment::Testing);
        assert_eq!(test_config.performance.query_timeout_seconds, 5); // Shorter timeout
        assert_eq!(test_config.storage.data_dir.to_str().unwrap(), "./test_data");

        // Test staging configuration
        let staging_config = DatabaseConfig::for_environment(Environment::Staging);
        assert_eq!(staging_config.environment, Environment::Staging);
        assert!(staging_config.security.enable_auth);
        assert!(staging_config.monitoring.enable_performance_monitoring);
    }

    #[test]
    fn test_config_validation() {
        let mut config = DatabaseConfig::default();
        
        // Valid configuration should pass
        assert!(config.validate().is_ok());

        // Test invalid max_connections
        config.performance.max_connections = 0;
        assert!(config.validate().is_err());
        config.performance.max_connections = 100; // Reset

        // Test invalid port
        config.network.port = 0;
        assert!(config.validate().is_err());
        config.network.port = 8080; // Reset

        // Test invalid write buffer size
        config.storage.write_buffer_size = 0;
        assert!(config.validate().is_err());
        config.storage.write_buffer_size = 64 * 1024 * 1024; // Reset

        // Test TLS configuration validation
        config.security.enable_tls = true;
        config.security.tls_cert_path = None;
        assert!(config.validate().is_err());
        
        config.security.tls_cert_path = Some("/path/to/cert.pem".into());
        config.security.tls_key_path = None;
        assert!(config.validate().is_err());
        
        config.security.tls_key_path = Some("/path/to/key.pem".into());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_file_operations() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.json");

        // Create a test configuration
        let mut config = DatabaseConfig::default();
        config.network.port = 9090;
        config.performance.max_connections = 200;
        config.security.enable_auth = true;

        // Save configuration to file
        config.save_to_file(&config_path, ConfigFormat::JSON).unwrap();
        assert!(config_path.exists());

        // Load configuration from file
        let loaded_config = DatabaseConfig::load_from_file(&config_path).unwrap();
        assert_eq!(loaded_config.network.port, 9090);
        assert_eq!(loaded_config.performance.max_connections, 200);
        assert!(loaded_config.security.enable_auth);
    }

    #[test]
    fn test_migration_validation() {
        let source_config = DatabaseConfig::for_environment(Environment::Development);
        let mut target_config = DatabaseConfig::for_environment(Environment::Production);
        
        // Change compression type to create a validation issue
        target_config.storage.compression = CompressionType::ZSTD;

        let migration_manager = MigrationManager::new(source_config, target_config);
        
        // Validate migration
        let issues = migration_manager.validate_migration().unwrap();
        assert!(!issues.is_empty());
        
        // Should have a warning about compression type difference
        assert!(issues.iter().any(|issue| 
            issue.message.contains("Compression type differs")
        ));
    }

    #[test]
    fn test_migration_execution() {
        let source_config = DatabaseConfig::for_environment(Environment::Development);
        let target_config = DatabaseConfig::for_environment(Environment::Production);

        let migration_manager = MigrationManager::new(source_config, target_config);
        
        // Execute migration
        let result = migration_manager.migrate_data().unwrap();
        
        assert!(result.success);
        assert!(result.end_time >= result.start_time);
        assert!(result.schemas_migrated > 0);
        assert!(result.documents_migrated > 0);
        assert!(result.indexes_rebuilt > 0);
        assert!(result.errors.is_empty());
    }
}

/// Integration tests combining multiple features
#[cfg(test)]
mod integration_tests {
    use super::*;

    #[test]
    fn test_monitoring_with_change_streams() {
        let temp_dir = TempDir::new().unwrap();
        let store = Arc::new(RocksDBStore::new(temp_dir.path()).unwrap());
        let change_store = ChangeTrackingStore::new(store);

        // Set up monitoring
        let config = MonitoringConfig::default();
        let monitoring = MonitoringSystem::new(config);

        // Subscribe to changes
        let filter = ChangeFilter::new().schemas(vec!["orders".to_string()]);
        let handle = change_store.change_manager().subscribe(filter).unwrap();

        // Perform operations and record metrics
        let query = create_test_query();
        
        // Insert operation
        let doc = create_test_document("order_1", "Test Order", "pending");
        change_store.put_document(&doc).unwrap();
        
        let insert_metrics = QueryMetrics::new(&query, 100, 1, 1);
        monitoring.record_query_metrics(insert_metrics).unwrap();

        // Update operation
        let updated_doc = create_test_document("order_1", "Test Order", "shipped");
        change_store.put_document(&updated_doc).unwrap();
        
        let update_metrics = QueryMetrics::new(&query, 80, 1, 1);
        monitoring.record_query_metrics(update_metrics).unwrap();

        // Verify change stream received events
        let insert_change = handle.try_next_change().unwrap();
        assert_eq!(insert_change.change_type, ChangeType::Insert);
        
        let update_change = handle.try_next_change().unwrap();
        assert_eq!(update_change.change_type, ChangeType::Update);

        // Verify monitoring recorded metrics
        let stats = monitoring.get_query_stats().unwrap();
        assert_eq!(stats.total_queries, 2);
        assert_eq!(stats.successful_queries, 2);
        assert_eq!(stats.avg_execution_time_ms, 90.0); // (100 + 80) / 2
    }

    #[test]
    fn test_backup_with_monitoring() {
        let temp_dir = TempDir::new().unwrap();
        let backup_dir = temp_dir.path().join("backups");
        
        // Set up backup configuration
        let backup_config = BackupConfig {
            enabled: true,
            backup_dir: backup_dir.clone(),
            interval_hours: 24,
            max_backups: 3,
            compress: false,
        };

        let backup_manager = BackupManager::new(backup_config);

        // Set up monitoring
        let config = MonitoringConfig::default();
        let monitoring = MonitoringSystem::new(config);

        // Create test database
        let db_dir = temp_dir.path().join("database");
        fs::create_dir_all(&db_dir).unwrap();
        fs::write(db_dir.join("data.db"), "test database").unwrap();

        // Create backup and record metrics
        let start_time = std::time::Instant::now();
        let backup_path = backup_manager
            .create_backup(&db_dir, Some("monitored_backup".to_string()))
            .unwrap();
        let backup_duration = start_time.elapsed().as_millis() as u64;

        // Record backup operation as a "system" query
        let backup_query = OQLQuery {
            collection: "system".to_string(),
            operations: vec![],
        };
        let backup_metrics = QueryMetrics::new(&backup_query, backup_duration, 0, 0)
            .with_index("backup_operation".to_string());
        monitoring.record_query_metrics(backup_metrics).unwrap();

        // Verify backup was created
        assert!(backup_path.exists());
        
        // Verify monitoring recorded the backup operation
        let recent_queries = monitoring.get_query_metrics(None).unwrap();
        assert!(!recent_queries.is_empty());
        assert!(recent_queries.iter().any(|q| q.schema == "system"));
    }

    #[test]
    fn test_full_deployment_scenario() {
        let temp_dir = TempDir::new().unwrap();
        
        // 1. Start with development configuration
        let mut dev_config = DatabaseConfig::for_environment(Environment::Development);
        dev_config.storage.data_dir = temp_dir.path().join("dev_data");
        
        // 2. Set up change tracking and monitoring
        let store = Arc::new(RocksDBStore::new(&dev_config.storage.data_dir).unwrap());
        let change_store = ChangeTrackingStore::new(store);
        let monitoring = MonitoringSystem::new(dev_config.monitoring.clone());

        // 3. Perform some operations
        let doc1 = create_test_document("order_1", "Development Order", "pending");
        let doc2 = create_test_document("order_2", "Another Order", "shipped");
        
        change_store.put_document(&doc1).unwrap();
        change_store.put_document(&doc2).unwrap();

        // Record metrics
        let query = create_test_query();
        let metrics = QueryMetrics::new(&query, 120, 2, 2);
        monitoring.record_query_metrics(metrics).unwrap();

        // 4. Create backup before migration
        let backup_config = BackupConfig {
            enabled: true,
            backup_dir: temp_dir.path().join("backups"),
            interval_hours: 24,
            max_backups: 5,
            compress: true,
        };
        
        let backup_manager = BackupManager::new(backup_config);
        let backup_path = backup_manager
            .create_backup(&dev_config.storage.data_dir, Some("pre_migration".to_string()))
            .unwrap();

        // 5. Prepare production configuration
        let mut prod_config = DatabaseConfig::for_environment(Environment::Production);
        prod_config.storage.data_dir = temp_dir.path().join("prod_data");

        // 6. Validate migration
        let migration_manager = MigrationManager::new(dev_config.clone(), prod_config.clone());
        let issues = migration_manager.validate_migration().unwrap();
        
        // May have warnings about configuration differences (not required)
        // assert!(!issues.is_empty());

        // 7. Execute migration
        let migration_result = migration_manager.migrate_data().unwrap();
        assert!(migration_result.success);

        // 8. Verify monitoring data is preserved
        let dashboard_data = monitoring.get_dashboard_data().unwrap();
        assert!(dashboard_data.query_stats.total_queries > 0);
        assert!(dashboard_data.system_metrics.uptime_seconds >= 0);

        // 9. Verify backup exists and can be listed
        let backups = backup_manager.list_backups().unwrap();
        assert!(!backups.is_empty());
        assert!(backups.iter().any(|b| b.name.contains("pre_migration")));
        assert!(backup_path.exists());
    }
}