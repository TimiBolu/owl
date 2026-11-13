// Change Streams and Triggers System for OQL Database

use crate::document::{Document, DocumentId, Value};
use crate::error::{OQLError, OQLResult};
use crate::parser::{Condition, Expression, OQLQuery};
use crate::storage::DocumentStore;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

/// Types of document changes that can be tracked
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ChangeType {
    Insert,
    Update,
    Delete,
}

/// Represents a change to a document
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentChange {
    pub change_id: String,
    pub document_id: DocumentId,
    pub schema: String,
    pub change_type: ChangeType,
    pub timestamp: DateTime<Utc>,
    pub old_document: Option<Document>,
    pub new_document: Option<Document>,
    pub changed_fields: Vec<String>,
}

impl DocumentChange {
    /// Create a new document change event
    pub fn new(
        document_id: DocumentId,
        schema: String,
        change_type: ChangeType,
        old_document: Option<Document>,
        new_document: Option<Document>,
    ) -> Self {
        let changed_fields = match (&old_document, &new_document) {
            (Some(old), Some(new)) => {
                // Find changed fields for updates
                let mut changed = Vec::new();
                for (field, new_value) in &new.fields {
                    if let Some(old_value) = old.fields.get(field) {
                        if old_value != new_value {
                            changed.push(field.clone());
                        }
                    } else {
                        changed.push(field.clone());
                    }
                }
                // Check for removed fields
                for field in old.fields.keys() {
                    if !new.fields.contains_key(field) {
                        changed.push(field.clone());
                    }
                }
                changed
            }
            _ => Vec::new(),
        };

        Self {
            change_id: uuid::Uuid::new_v4().to_string(),
            document_id,
            schema,
            change_type,
            timestamp: Utc::now(),
            old_document,
            new_document,
            changed_fields,
        }
    }

    /// Get the document after the change (for insert/update)
    pub fn document(&self) -> Option<&Document> {
        self.new_document.as_ref()
    }

    /// Get the field value from the new document
    pub fn get_field(&self, field_name: &str) -> Option<&Value> {
        self.new_document.as_ref()?.get_field(field_name)
    }

    /// Check if a specific field was changed
    pub fn field_changed(&self, field_name: &str) -> bool {
        self.changed_fields.contains(&field_name.to_string())
    }
}

/// Filter for change stream subscriptions
#[derive(Debug, Clone)]
pub struct ChangeFilter {
    pub schemas: Option<Vec<String>>,
    pub change_types: Option<Vec<ChangeType>>,
    pub field_filters: Option<HashMap<String, Value>>,
    pub condition: Option<Condition>,
}

impl ChangeFilter {
    /// Create a new change filter
    pub fn new() -> Self {
        Self {
            schemas: None,
            change_types: None,
            field_filters: None,
            condition: None,
        }
    }

    /// Filter by specific schemas
    pub fn schemas(mut self, schemas: Vec<String>) -> Self {
        self.schemas = Some(schemas);
        self
    }

    /// Filter by change types
    pub fn change_types(mut self, types: Vec<ChangeType>) -> Self {
        self.change_types = Some(types);
        self
    }

    /// Filter by field values
    pub fn field_filter(mut self, field: String, value: Value) -> Self {
        if self.field_filters.is_none() {
            self.field_filters = Some(HashMap::new());
        }
        self.field_filters.as_mut().unwrap().insert(field, value);
        self
    }

    /// Filter by custom condition
    pub fn condition(mut self, condition: Condition) -> Self {
        self.condition = Some(condition);
        self
    }

    /// Check if a change matches this filter
    pub fn matches(&self, change: &DocumentChange) -> OQLResult<bool> {
        // Check schema filter
        if let Some(schemas) = &self.schemas {
            if !schemas.contains(&change.schema) {
                return Ok(false);
            }
        }

        // Check change type filter
        if let Some(types) = &self.change_types {
            if !types.contains(&change.change_type) {
                return Ok(false);
            }
        }

        // Check field filters
        if let Some(field_filters) = &self.field_filters {
            for (field, expected_value) in field_filters {
                if let Some(actual_value) = change.get_field(field) {
                    if actual_value != expected_value {
                        return Ok(false);
                    }
                } else {
                    return Ok(false);
                }
            }
        }

        // Check custom condition
        if let Some(_condition) = &self.condition {
            // For now, we'll implement a simplified condition check
            // In a full implementation, we'd need to evaluate the condition against the change
            // This would require extending the executor to work with DocumentChange
            return Ok(true); // Simplified - always matches for now
        }

        Ok(true)
    }
}

impl Default for ChangeFilter {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle for a change stream subscription
pub struct ChangeStreamHandle {
    pub id: String,
    pub receiver: mpsc::Receiver<DocumentChange>,
    _sender: mpsc::Sender<DocumentChange>, // Keep sender alive
}

impl ChangeStreamHandle {
    /// Get the next change from the stream (blocking)
    pub fn next_change(&self) -> Option<DocumentChange> {
        self.receiver.recv().ok()
    }

    /// Try to get the next change without blocking
    pub fn try_next_change(&self) -> Option<DocumentChange> {
        self.receiver.try_recv().ok()
    }
}

/// Trigger action to execute when conditions are met
#[derive(Debug, Clone)]
pub enum TriggerAction {
    /// Execute an OQL query
    Query(OQLQuery),
    /// Call a custom function
    Function { name: String, args: Vec<Expression> },
    /// Send a notification
    Notification {
        message: String,
        recipients: Vec<String>,
    },
}

/// Trigger definition
#[derive(Debug, Clone)]
pub struct Trigger {
    pub id: String,
    pub name: String,
    pub schema: String,
    pub change_types: Vec<ChangeType>,
    pub condition: Option<Condition>,
    pub action: TriggerAction,
    pub enabled: bool,
}

impl Trigger {
    /// Create a new trigger
    pub fn new(
        name: String,
        schema: String,
        change_types: Vec<ChangeType>,
        action: TriggerAction,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            schema,
            change_types,
            condition: None,
            action,
            enabled: true,
        }
    }

    /// Add a condition to the trigger
    pub fn with_condition(mut self, condition: Condition) -> Self {
        self.condition = Some(condition);
        self
    }

    /// Check if this trigger should fire for a given change
    pub fn should_fire(&self, change: &DocumentChange) -> OQLResult<bool> {
        if !self.enabled {
            return Ok(false);
        }

        // Check schema match
        if change.schema != self.schema {
            return Ok(false);
        }

        // Check change type match
        if !self.change_types.contains(&change.change_type) {
            return Ok(false);
        }

        // Check condition if present
        if let Some(_condition) = &self.condition {
            // For now, we'll implement a simplified condition check
            // In a full implementation, we'd need to evaluate the condition against the change
            return Ok(true); // Simplified - always matches for now
        }

        Ok(true)
    }
}

/// Change streams and triggers manager
type SubscriberMap = HashMap<String, (ChangeFilter, mpsc::Sender<DocumentChange>)>;

pub struct ChangeStreamManager<S: DocumentStore> {
    store: Arc<S>,
    subscribers: Arc<Mutex<SubscriberMap>>,
    triggers: Arc<Mutex<HashMap<String, Trigger>>>,
    change_log: Arc<Mutex<Vec<DocumentChange>>>,
    max_change_log_size: usize,
}

impl<S: DocumentStore + Send + Sync + 'static> ChangeStreamManager<S> {
    /// Create a new change stream manager
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store,
            subscribers: Arc::new(Mutex::new(HashMap::new())),
            triggers: Arc::new(Mutex::new(HashMap::new())),
            change_log: Arc::new(Mutex::new(Vec::new())),
            max_change_log_size: 10000, // Keep last 10k changes
        }
    }

    /// Record a document change and notify subscribers
    pub fn record_change(&self, change: DocumentChange) -> OQLResult<()> {
        // Add to change log
        {
            let mut log = self.change_log.lock().map_err(|_| OQLError::Generic {
                message: "Failed to acquire change log lock".to_string(),
            })?;

            log.push(change.clone());

            // Trim log if it gets too large
            if log.len() > self.max_change_log_size {
                let excess = log.len() - self.max_change_log_size;
                log.drain(0..excess);
            }
        }

        // Notify subscribers
        {
            let subscribers = self.subscribers.lock().map_err(|_| OQLError::Generic {
                message: "Failed to acquire subscribers lock".to_string(),
            })?;

            for (filter, sender) in subscribers.values() {
                if filter.matches(&change)? {
                    // Send change to subscriber (ignore if channel is closed)
                    let _ = sender.send(change.clone());
                }
            }
        }

        // Check triggers
        self.check_triggers(&change)?;

        Ok(())
    }

    /// Subscribe to change stream with a filter
    pub fn subscribe(&self, filter: ChangeFilter) -> OQLResult<ChangeStreamHandle> {
        let (sender, receiver) = mpsc::channel();
        let handle_id = uuid::Uuid::new_v4().to_string();

        {
            let mut subscribers = self.subscribers.lock().map_err(|_| OQLError::Generic {
                message: "Failed to acquire subscribers lock".to_string(),
            })?;

            subscribers.insert(handle_id.clone(), (filter, sender.clone()));
        }

        Ok(ChangeStreamHandle {
            id: handle_id,
            receiver,
            _sender: sender,
        })
    }

    /// Unsubscribe from change stream
    pub fn unsubscribe(&self, handle_id: &str) -> OQLResult<()> {
        let mut subscribers = self.subscribers.lock().map_err(|_| OQLError::Generic {
            message: "Failed to acquire subscribers lock".to_string(),
        })?;

        subscribers.remove(handle_id);
        Ok(())
    }

    /// Register a trigger
    pub fn register_trigger(&self, trigger: Trigger) -> OQLResult<String> {
        let trigger_id = trigger.id.clone();

        let mut triggers = self.triggers.lock().map_err(|_| OQLError::Generic {
            message: "Failed to acquire triggers lock".to_string(),
        })?;

        triggers.insert(trigger_id.clone(), trigger);
        Ok(trigger_id)
    }

    /// Remove a trigger
    pub fn remove_trigger(&self, trigger_id: &str) -> OQLResult<bool> {
        let mut triggers = self.triggers.lock().map_err(|_| OQLError::Generic {
            message: "Failed to acquire triggers lock".to_string(),
        })?;

        Ok(triggers.remove(trigger_id).is_some())
    }

    /// List all triggers
    pub fn list_triggers(&self) -> OQLResult<Vec<Trigger>> {
        let triggers = self.triggers.lock().map_err(|_| OQLError::Generic {
            message: "Failed to acquire triggers lock".to_string(),
        })?;

        Ok(triggers.values().cloned().collect())
    }

    /// Enable or disable a trigger
    pub fn set_trigger_enabled(&self, trigger_id: &str, enabled: bool) -> OQLResult<bool> {
        let mut triggers = self.triggers.lock().map_err(|_| OQLError::Generic {
            message: "Failed to acquire triggers lock".to_string(),
        })?;

        if let Some(trigger) = triggers.get_mut(trigger_id) {
            trigger.enabled = enabled;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Get change history
    pub fn get_change_history(&self, limit: Option<usize>) -> OQLResult<Vec<DocumentChange>> {
        let log = self.change_log.lock().map_err(|_| OQLError::Generic {
            message: "Failed to acquire change log lock".to_string(),
        })?;

        let changes = if let Some(limit) = limit {
            log.iter().rev().take(limit).cloned().collect()
        } else {
            log.clone()
        };

        Ok(changes)
    }

    /// Check and execute triggers for a change
    fn check_triggers(&self, change: &DocumentChange) -> OQLResult<()> {
        let triggers = self.triggers.lock().map_err(|_| OQLError::Generic {
            message: "Failed to acquire triggers lock".to_string(),
        })?;

        for trigger in triggers.values() {
            if trigger.should_fire(change)? {
                // Execute trigger action in a separate thread to avoid blocking
                let trigger_clone = trigger.clone();
                let change_clone = change.clone();
                let store_clone = self.store.clone();

                thread::spawn(move || {
                    if let Err(e) =
                        Self::execute_trigger_action(&store_clone, &trigger_clone, &change_clone)
                    {
                        eprintln!("Trigger execution failed: {:?}", e);
                    }
                });
            }
        }

        Ok(())
    }

    /// Execute a trigger action
    fn execute_trigger_action(
        _store: &Arc<S>,
        trigger: &Trigger,
        change: &DocumentChange,
    ) -> OQLResult<()> {
        match &trigger.action {
            TriggerAction::Query(_query) => {
                // In a full implementation, we'd execute the query here
                // For now, just log the action
                println!(
                    "Trigger '{}' would execute query for change: {:?}",
                    trigger.name, change.change_id
                );
                Ok(())
            }
            TriggerAction::Function { name, args: _ } => {
                // In a full implementation, we'd call the function here
                println!(
                    "Trigger '{}' would call function '{}' for change: {:?}",
                    trigger.name, name, change.change_id
                );
                Ok(())
            }
            TriggerAction::Notification {
                message,
                recipients,
            } => {
                // In a full implementation, we'd send notifications here
                println!(
                    "Trigger '{}' would send notification '{}' to {:?} for change: {:?}",
                    trigger.name, message, recipients, change.change_id
                );
                Ok(())
            }
        }
    }
}

/// Wrapper for DocumentStore that automatically records changes
pub struct ChangeTrackingStore<S: DocumentStore> {
    inner: Arc<S>,
    change_manager: Arc<ChangeStreamManager<S>>,
}

impl<S: DocumentStore + Send + Sync + 'static> ChangeTrackingStore<S> {
    /// Create a new change tracking store
    pub fn new(store: Arc<S>) -> Self {
        let change_manager = Arc::new(ChangeStreamManager::new(store.clone()));
        Self {
            inner: store,
            change_manager,
        }
    }

    /// Get access to the change stream manager
    pub fn change_manager(&self) -> &Arc<ChangeStreamManager<S>> {
        &self.change_manager
    }

    /// Get access to the inner store
    pub fn inner(&self) -> &Arc<S> {
        &self.inner
    }
}

impl<S: DocumentStore + Send + Sync + 'static> DocumentStore for ChangeTrackingStore<S> {
    fn get_document(&self, schema: &str, id: &DocumentId) -> OQLResult<Option<Document>> {
        self.inner.get_document(schema, id)
    }

    fn put_document(&self, document: &Document) -> OQLResult<()> {
        // Get existing document for change tracking
        let old_document = self.inner.get_document(&document.schema, &document.id)?;

        // Store the document
        self.inner.put_document(document)?;

        // Record the change
        let change_type = if old_document.is_some() {
            ChangeType::Update
        } else {
            ChangeType::Insert
        };

        let change = DocumentChange::new(
            document.id.clone(),
            document.schema.clone(),
            change_type,
            old_document,
            Some(document.clone()),
        );

        self.change_manager.record_change(change)?;

        Ok(())
    }

    fn delete_document(&self, schema: &str, id: &DocumentId) -> OQLResult<bool> {
        // Get existing document for change tracking
        let old_document = self.inner.get_document(schema, id)?;

        // Delete the document
        let was_deleted = self.inner.delete_document(schema, id)?;

        // Record the change if document was actually deleted
        if was_deleted && old_document.is_some() {
            let change = DocumentChange::new(
                id.clone(),
                schema.to_string(),
                ChangeType::Delete,
                old_document,
                None,
            );

            self.change_manager.record_change(change)?;
        }

        Ok(was_deleted)
    }

    fn scan_documents(&self, schema: &str) -> OQLResult<Vec<Document>> {
        self.inner.scan_documents(schema)
    }

    fn get_schema(&self, name: &str) -> OQLResult<Option<crate::schema::Schema>> {
        self.inner.get_schema(name)
    }

    fn put_schema(&self, schema: &crate::schema::Schema) -> OQLResult<()> {
        self.inner.put_schema(schema)
    }

    fn list_schemas(&self) -> OQLResult<Vec<String>> {
        self.inner.list_schemas()
    }

    fn navigate_reference(&self, objref: &crate::document::ObjRef) -> OQLResult<Option<Document>> {
        self.inner.navigate_reference(objref)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::RustStore;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn create_test_store() -> (ChangeTrackingStore<RustStore>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("test.owl");
        let store = Arc::new(RustStore::new(path.to_str().unwrap(), 10 * 1024 * 1024).unwrap());
        let change_store = ChangeTrackingStore::new(store);
        (change_store, temp_dir)
    }

    fn create_test_document(id: &str, name: &str) -> Document {
        let mut fields = HashMap::new();
        fields.insert("name".to_string(), Value::String(name.to_string()));
        Document::with_id(id.to_string(), "users".to_string(), fields)
    }

    #[test]
    fn test_change_tracking_insert() {
        let (store, _temp_dir) = create_test_store();
        let doc = create_test_document("user_1", "Alice");

        // Insert document
        store.put_document(&doc).unwrap();

        // Check change history
        let changes = store.change_manager().get_change_history(None).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change_type, ChangeType::Insert);
        assert_eq!(changes[0].document_id, "user_1");
        assert!(changes[0].old_document.is_none());
        assert!(changes[0].new_document.is_some());
    }

    #[test]
    fn test_change_tracking_update() {
        let (store, _temp_dir) = create_test_store();
        let doc1 = create_test_document("user_1", "Alice");
        let doc2 = create_test_document("user_1", "Alice Smith");

        // Insert then update
        store.put_document(&doc1).unwrap();
        store.put_document(&doc2).unwrap();

        // Check change history
        let changes = store.change_manager().get_change_history(None).unwrap();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].change_type, ChangeType::Insert);
        assert_eq!(changes[1].change_type, ChangeType::Update);
        assert!(changes[1].field_changed("name"));
    }

    #[test]
    fn test_change_tracking_delete() {
        let (store, _temp_dir) = create_test_store();
        let doc = create_test_document("user_1", "Alice");

        // Insert then delete
        store.put_document(&doc).unwrap();
        store
            .delete_document("users", &"user_1".to_string())
            .unwrap();

        // Check change history
        let changes = store.change_manager().get_change_history(None).unwrap();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].change_type, ChangeType::Insert);
        assert_eq!(changes[1].change_type, ChangeType::Delete);
        assert!(changes[1].new_document.is_none());
        assert!(changes[1].old_document.is_some());
    }

    #[test]
    fn test_change_stream_subscription() {
        let (store, _temp_dir) = create_test_store();

        // Subscribe to changes
        let filter = ChangeFilter::new().schemas(vec!["users".to_string()]);
        let handle = store.change_manager().subscribe(filter).unwrap();

        // Insert a document
        let doc = create_test_document("user_1", "Alice");
        store.put_document(&doc).unwrap();

        // Check if we received the change
        let change = handle.try_next_change();
        assert!(change.is_some());
        let change = change.unwrap();
        assert_eq!(change.change_type, ChangeType::Insert);
        assert_eq!(change.document_id, "user_1");
    }

    #[test]
    fn test_trigger_registration() {
        let (store, _temp_dir) = create_test_store();

        // Register a trigger
        let trigger = Trigger::new(
            "test_trigger".to_string(),
            "users".to_string(),
            vec![ChangeType::Insert],
            TriggerAction::Notification {
                message: "New user created".to_string(),
                recipients: vec!["admin@example.com".to_string()],
            },
        );

        let trigger_id = store.change_manager().register_trigger(trigger).unwrap();

        // List triggers
        let triggers = store.change_manager().list_triggers().unwrap();
        assert_eq!(triggers.len(), 1);
        assert_eq!(triggers[0].name, "test_trigger");

        // Remove trigger
        let removed = store.change_manager().remove_trigger(&trigger_id).unwrap();
        assert!(removed);

        let triggers = store.change_manager().list_triggers().unwrap();
        assert_eq!(triggers.len(), 0);
    }
}
