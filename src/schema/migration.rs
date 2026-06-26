use crate::document::{Document, Value};
use crate::error::{OQLError, OQLResult};
use crate::schema::evolution::{FieldChange, SchemaVersion};
use crate::schema::FieldType;
use crate::storage::DocumentStore;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Migration step for transforming data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MigrationStep {
    /// Add a field with a default value
    AddField {
        field_name: String,
        default_value: Value,
    },
    /// Remove a field
    RemoveField { field_name: String },
    /// Rename a field
    RenameField { old_name: String, new_name: String },
    /// Transform a field value
    TransformField {
        field_name: String,
        transform: FieldTransform,
    },
    /// Copy a field to a new name
    CopyField { source: String, destination: String },
    /// Custom transformation function
    Custom {
        description: String,
        // In a real implementation, this would be a function pointer or script
    },
}

/// Field transformation types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FieldTransform {
    /// Convert to string
    ToString,
    /// Convert to integer
    ToInteger,
    /// Convert to float
    ToFloat,
    /// Convert to boolean
    ToBoolean,
    /// Make optional (wrap in Option)
    MakeOptional,
    /// Unwrap optional
    UnwrapOptional { default_value: Value },
    /// Apply a constant value
    Constant { value: Value },
}

/// Migration script with steps
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationScript {
    pub id: String,
    pub from_version: u32,
    pub to_version: u32,
    pub schema_name: String,
    pub description: String,
    pub steps: Vec<MigrationStep>,
    pub created_at: DateTime<Utc>,
}

impl MigrationScript {
    /// Create a new migration script
    pub fn new(
        schema_name: String,
        from_version: u32,
        to_version: u32,
        description: String,
    ) -> Self {
        Self {
            id: format!("{}_{}_to_{}", schema_name, from_version, to_version),
            from_version,
            to_version,
            schema_name,
            description,
            steps: Vec::new(),
            created_at: Utc::now(),
        }
    }

    /// Add a migration step
    pub fn add_step(&mut self, step: MigrationStep) {
        self.steps.push(step);
    }

    /// Generate migration steps from field changes
    pub fn from_changes(
        schema_name: String,
        from_version: u32,
        to_version: u32,
        changes: Vec<FieldChange>,
    ) -> Self {
        let mut script = Self::new(
            schema_name,
            from_version,
            to_version,
            "Auto-generated migration".to_string(),
        );

        for change in changes {
            match change {
                FieldChange::AddField {
                    field_name,
                    field_type,
                    default_value,
                } => {
                    let default = default_value
                        .and_then(|v| Self::parse_default_value(&v, &field_type))
                        .unwrap_or(Value::Null);

                    script.add_step(MigrationStep::AddField {
                        field_name,
                        default_value: default,
                    });
                }
                FieldChange::RemoveField { field_name } => {
                    script.add_step(MigrationStep::RemoveField { field_name });
                }
                FieldChange::RenameField { old_name, new_name } => {
                    script.add_step(MigrationStep::RenameField { old_name, new_name });
                }
                FieldChange::ChangeType {
                    field_name,
                    old_type,
                    new_type,
                } => {
                    if let Some(transform) = Self::infer_transform(&old_type, &new_type) {
                        script.add_step(MigrationStep::TransformField {
                            field_name,
                            transform,
                        });
                    }
                }
                FieldChange::MakeOptional { field_name } => {
                    script.add_step(MigrationStep::TransformField {
                        field_name,
                        transform: FieldTransform::MakeOptional,
                    });
                }
                FieldChange::MakeRequired {
                    field_name,
                    default_value,
                } => {
                    let default = default_value
                        .and_then(|v| serde_json::from_str(&v).ok())
                        .unwrap_or(Value::Null);

                    script.add_step(MigrationStep::TransformField {
                        field_name,
                        transform: FieldTransform::UnwrapOptional {
                            default_value: default,
                        },
                    });
                }
                FieldChange::ChangeReferenceTarget { .. } => {
                    // Reference target changes require custom handling
                    script.add_step(MigrationStep::Custom {
                        description: "Reference target change requires manual migration"
                            .to_string(),
                    });
                }
            }
        }

        script
    }

    /// Parse a default value string based on field type
    fn parse_default_value(value_str: &str, field_type: &FieldType) -> Option<Value> {
        match field_type {
            FieldType::String => Some(Value::String(value_str.to_string())),
            FieldType::Integer => value_str.parse::<i64>().ok().map(Value::Integer),
            FieldType::Float => value_str.parse::<f64>().ok().map(Value::Float),
            FieldType::Boolean => value_str.parse::<bool>().ok().map(Value::Boolean),
            _ => None,
        }
    }

    /// Infer transformation from type change
    fn infer_transform(old_type: &FieldType, new_type: &FieldType) -> Option<FieldTransform> {
        match (old_type, new_type) {
            (_, FieldType::String) => Some(FieldTransform::ToString),
            (FieldType::String, FieldType::Integer) => Some(FieldTransform::ToInteger),
            (FieldType::Integer, FieldType::Float) => Some(FieldTransform::ToFloat),
            (FieldType::String, FieldType::Boolean) => Some(FieldTransform::ToBoolean),
            _ => None,
        }
    }

    /// Apply this migration to a document
    pub fn apply_to_document(&self, doc: &mut Document) -> OQLResult<()> {
        for step in &self.steps {
            self.apply_step(doc, step)?;
        }
        Ok(())
    }

    /// Apply a single migration step to a document
    fn apply_step(&self, doc: &mut Document, step: &MigrationStep) -> OQLResult<()> {
        match step {
            MigrationStep::AddField {
                field_name,
                default_value,
            } => {
                if !doc.fields.contains_key(field_name) {
                    doc.fields.insert(field_name.clone(), default_value.clone());
                }
            }
            MigrationStep::RemoveField { field_name } => {
                doc.fields.remove(field_name);
            }
            MigrationStep::RenameField { old_name, new_name } => {
                if let Some(value) = doc.fields.remove(old_name) {
                    doc.fields.insert(new_name.clone(), value);
                }
            }
            MigrationStep::TransformField {
                field_name,
                transform,
            } => {
                if let Some(value) = doc.fields.get_mut(field_name) {
                    *value = self.apply_transform(value, transform)?;
                }
            }
            MigrationStep::CopyField {
                source,
                destination,
            } => {
                if let Some(value) = doc.fields.get(source) {
                    doc.fields.insert(destination.clone(), value.clone());
                }
            }
            MigrationStep::Custom { description } => {
                return Err(OQLError::Generic {
                    message: format!("Custom migration step not implemented: {}", description),
                });
            }
        }
        Ok(())
    }

    /// Apply a field transformation
    fn apply_transform(&self, value: &Value, transform: &FieldTransform) -> OQLResult<Value> {
        match transform {
            FieldTransform::ToString => {
                // Convert value to string representation
                let string_val = match value {
                    Value::String(s) => s.clone(),
                    Value::Integer(i) => i.to_string(),
                    Value::Float(f) => f.to_string(),
                    Value::Boolean(b) => b.to_string(),
                    Value::Null => "null".to_string(),
                    Value::List(_) => "[list]".to_string(),
                    Value::DateTime(dt) => dt.to_rfc3339(),
                    Value::Reference(_) => "[reference]".to_string(),
                    Value::GlobalReference(_) => "[reference]".to_string(),
                    Value::Object(_) => "[object]".to_string(),
                };
                Ok(Value::String(string_val))
            }
            FieldTransform::ToInteger => match value {
                Value::Integer(i) => Ok(Value::Integer(*i)),
                Value::Float(f) => Ok(Value::Integer(*f as i64)),
                Value::String(s) => {
                    s.parse::<i64>()
                        .map(Value::Integer)
                        .map_err(|_| OQLError::Generic {
                            message: format!("Cannot convert '{}' to integer", s),
                        })
                }
                _ => Err(OQLError::TypeError {
                    expected: "Integer".to_string(),
                    actual: value.type_name().to_string(),
                }),
            },
            FieldTransform::ToFloat => match value {
                Value::Float(f) => Ok(Value::Float(*f)),
                Value::Integer(i) => Ok(Value::Float(*i as f64)),
                Value::String(s) => {
                    s.parse::<f64>()
                        .map(Value::Float)
                        .map_err(|_| OQLError::Generic {
                            message: format!("Cannot convert '{}' to float", s),
                        })
                }
                _ => Err(OQLError::TypeError {
                    expected: "Float".to_string(),
                    actual: value.type_name().to_string(),
                }),
            },
            FieldTransform::ToBoolean => match value {
                Value::Boolean(b) => Ok(Value::Boolean(*b)),
                Value::String(s) => match s.to_lowercase().as_str() {
                    "true" | "yes" | "1" => Ok(Value::Boolean(true)),
                    "false" | "no" | "0" => Ok(Value::Boolean(false)),
                    _ => Err(OQLError::Generic {
                        message: format!("Cannot convert '{}' to boolean", s),
                    }),
                },
                Value::Integer(i) => Ok(Value::Boolean(*i != 0)),
                _ => Err(OQLError::TypeError {
                    expected: "Boolean".to_string(),
                    actual: value.type_name().to_string(),
                }),
            },
            FieldTransform::MakeOptional => Ok(value.clone()),
            FieldTransform::UnwrapOptional { default_value } => {
                if value.is_null() {
                    Ok(default_value.clone())
                } else {
                    Ok(value.clone())
                }
            }
            FieldTransform::Constant { value } => Ok(value.clone()),
        }
    }
}

/// Migration plan with multiple scripts
#[derive(Debug, Clone)]
pub struct MigrationPlan {
    pub schema_name: String,
    pub from_version: u32,
    pub to_version: u32,
    pub scripts: Vec<MigrationScript>,
}

impl MigrationPlan {
    /// Create a new migration plan
    pub fn new(schema_name: String, from_version: u32, to_version: u32) -> Self {
        Self {
            schema_name,
            from_version,
            to_version,
            scripts: Vec::new(),
        }
    }

    /// Add a migration script to the plan
    pub fn add_script(&mut self, script: MigrationScript) {
        self.scripts.push(script);
    }

    /// Generate a migration plan from schema versions
    pub fn from_versions(
        from_version: &SchemaVersion,
        to_version: &SchemaVersion,
        intermediate_versions: Vec<&SchemaVersion>,
    ) -> Self {
        let mut plan = Self::new(
            from_version.schema_name.clone(),
            from_version.version,
            to_version.version,
        );

        // Create migration scripts for each version step
        let mut versions = vec![from_version];
        versions.extend(intermediate_versions);
        versions.push(to_version);

        for i in 0..versions.len() - 1 {
            let from = versions[i];
            let to = versions[i + 1];

            let script = MigrationScript::from_changes(
                from.schema_name.clone(),
                from.version,
                to.version,
                to.changes.clone(),
            );

            plan.add_script(script);
        }

        plan
    }

    /// Apply the migration plan to a document
    pub fn apply_to_document(&self, doc: &mut Document) -> OQLResult<()> {
        for script in &self.scripts {
            script.apply_to_document(doc)?;
        }
        Ok(())
    }
}

/// Migration status tracking
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MigrationStatus {
    Pending,
    InProgress {
        started_at: DateTime<Utc>,
        progress: f64,
    },
    Completed {
        completed_at: DateTime<Utc>,
        documents_migrated: usize,
    },
    Failed {
        failed_at: DateTime<Utc>,
        error: String,
    },
}

/// Migration record for tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Migration {
    pub id: String,
    pub schema_name: String,
    pub from_version: u32,
    pub to_version: u32,
    pub status: MigrationStatus,
    pub created_at: DateTime<Utc>,
}

impl Migration {
    /// Create a new migration record
    pub fn new(schema_name: String, from_version: u32, to_version: u32) -> Self {
        Self {
            id: format!(
                "migration_{}_{}_to_{}",
                schema_name, from_version, to_version
            ),
            schema_name,
            from_version,
            to_version,
            status: MigrationStatus::Pending,
            created_at: Utc::now(),
        }
    }

    /// Mark migration as in progress
    pub fn start(&mut self) {
        self.status = MigrationStatus::InProgress {
            started_at: Utc::now(),
            progress: 0.0,
        };
    }

    /// Update migration progress
    pub fn update_progress(&mut self, progress: f64) {
        if let MigrationStatus::InProgress { started_at, .. } = self.status {
            self.status = MigrationStatus::InProgress {
                started_at,
                progress,
            };
        }
    }

    /// Mark migration as completed
    pub fn complete(&mut self, documents_migrated: usize) {
        self.status = MigrationStatus::Completed {
            completed_at: Utc::now(),
            documents_migrated,
        };
    }

    /// Mark migration as failed
    pub fn fail(&mut self, error: String) {
        self.status = MigrationStatus::Failed {
            failed_at: Utc::now(),
            error,
        };
    }
}

/// Migration executor for applying migrations to a collection
pub struct MigrationExecutor<S: DocumentStore> {
    store: S,
}

impl<S: DocumentStore> MigrationExecutor<S> {
    /// Create a new migration executor
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// Execute a migration plan on a collection (schema)
    pub fn execute(
        &mut self,
        schema: &str,
        plan: &MigrationPlan,
        mut migration: Migration,
    ) -> OQLResult<Migration> {
        migration.start();

        // Get all documents in the schema
        let documents = self.store.scan_documents(schema)?;
        let total_docs = documents.len();

        let mut migrated_count = 0;

        for (index, mut doc) in documents.into_iter().enumerate() {
            // Apply migration
            if let Err(e) = plan.apply_to_document(&mut doc) {
                migration.fail(format!("Failed to migrate document {}: {}", doc.id, e));
                return Ok(migration);
            }

            // Write back
            if let Err(e) = self.store.put_document(&doc) {
                migration.fail(format!("Failed to update document {}: {}", doc.id, e));
                return Ok(migration);
            }

            migrated_count += 1;

            // Update progress
            let progress = (index + 1) as f64 / total_docs as f64;
            migration.update_progress(progress);
        }

        migration.complete(migrated_count);
        Ok(migration)
    }

    /// Dry run - validate migration without applying changes
    pub fn dry_run(&self, schema: &str, plan: &MigrationPlan) -> OQLResult<Vec<String>> {
        let mut issues = Vec::new();
        let documents = self.store.scan_documents(schema)?;

        for doc in documents.iter().take(10) {
            // Sample first 10 documents
            let mut doc_clone = doc.clone();

            if let Err(e) = plan.apply_to_document(&mut doc_clone) {
                issues.push(format!("Document {}: {}", doc.id, e));
            }
        }

        Ok(issues)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_migration_script_creation() {
        let script = MigrationScript::new("users".to_string(), 1, 2, "Add email field".to_string());

        assert_eq!(script.from_version, 1);
        assert_eq!(script.to_version, 2);
        assert_eq!(script.schema_name, "users");
        assert!(script.steps.is_empty());
    }

    #[test]
    fn test_add_field_migration() {
        let mut script =
            MigrationScript::new("users".to_string(), 1, 2, "Add email field".to_string());

        script.add_step(MigrationStep::AddField {
            field_name: "email".to_string(),
            default_value: Value::String("unknown@example.com".to_string()),
        });

        let mut doc = Document::with_id("user1".to_string(), "users".to_string(), HashMap::new());
        doc.fields
            .insert("name".to_string(), Value::String("Alice".to_string()));

        script.apply_to_document(&mut doc).unwrap();

        assert_eq!(doc.fields.len(), 2);
        assert_eq!(
            doc.fields.get("email"),
            Some(&Value::String("unknown@example.com".to_string()))
        );
    }

    #[test]
    fn test_remove_field_migration() {
        let mut script =
            MigrationScript::new("users".to_string(), 1, 2, "Remove old field".to_string());

        script.add_step(MigrationStep::RemoveField {
            field_name: "old_field".to_string(),
        });

        let mut doc = Document::with_id("user1".to_string(), "users".to_string(), HashMap::new());
        doc.fields
            .insert("name".to_string(), Value::String("Alice".to_string()));
        doc.fields.insert(
            "old_field".to_string(),
            Value::String("deprecated".to_string()),
        );

        script.apply_to_document(&mut doc).unwrap();

        assert_eq!(doc.fields.len(), 1);
        assert!(!doc.fields.contains_key("old_field"));
    }

    #[test]
    fn test_rename_field_migration() {
        let mut script =
            MigrationScript::new("users".to_string(), 1, 2, "Rename field".to_string());

        script.add_step(MigrationStep::RenameField {
            old_name: "email".to_string(),
            new_name: "email_address".to_string(),
        });

        let mut doc = Document::with_id("user1".to_string(), "users".to_string(), HashMap::new());
        doc.fields
            .insert("name".to_string(), Value::String("Alice".to_string()));
        doc.fields.insert(
            "email".to_string(),
            Value::String("alice@example.com".to_string()),
        );

        script.apply_to_document(&mut doc).unwrap();

        assert!(!doc.fields.contains_key("email"));
        assert_eq!(
            doc.fields.get("email_address"),
            Some(&Value::String("alice@example.com".to_string()))
        );
    }

    #[test]
    fn test_transform_to_string() {
        let mut script =
            MigrationScript::new("users".to_string(), 1, 2, "Transform field".to_string());

        script.add_step(MigrationStep::TransformField {
            field_name: "age".to_string(),
            transform: FieldTransform::ToString,
        });

        let mut doc = Document::with_id("user1".to_string(), "users".to_string(), HashMap::new());
        doc.fields.insert("age".to_string(), Value::Integer(30));

        script.apply_to_document(&mut doc).unwrap();

        assert_eq!(
            doc.fields.get("age"),
            Some(&Value::String("30".to_string()))
        );
    }

    #[test]
    fn test_transform_to_integer() {
        let mut script =
            MigrationScript::new("users".to_string(), 1, 2, "Transform field".to_string());

        script.add_step(MigrationStep::TransformField {
            field_name: "age".to_string(),
            transform: FieldTransform::ToInteger,
        });

        let mut doc = Document::with_id("user1".to_string(), "users".to_string(), HashMap::new());
        doc.fields
            .insert("age".to_string(), Value::String("30".to_string()));

        script.apply_to_document(&mut doc).unwrap();

        assert_eq!(doc.fields.get("age"), Some(&Value::Integer(30)));
    }

    #[test]
    fn test_migration_plan() {
        let mut plan = MigrationPlan::new("users".to_string(), 1, 3);

        let mut script1 = MigrationScript::new("users".to_string(), 1, 2, "Step 1".to_string());
        script1.add_step(MigrationStep::AddField {
            field_name: "email".to_string(),
            default_value: Value::Null,
        });

        let mut script2 = MigrationScript::new("users".to_string(), 2, 3, "Step 2".to_string());
        script2.add_step(MigrationStep::AddField {
            field_name: "phone".to_string(),
            default_value: Value::Null,
        });

        plan.add_script(script1);
        plan.add_script(script2);

        let mut doc = Document::with_id("user1".to_string(), "users".to_string(), HashMap::new());
        doc.fields
            .insert("name".to_string(), Value::String("Alice".to_string()));

        plan.apply_to_document(&mut doc).unwrap();

        assert_eq!(doc.fields.len(), 3);
        assert!(doc.fields.contains_key("email"));
        assert!(doc.fields.contains_key("phone"));
    }

    #[test]
    fn test_migration_status() {
        let mut migration = Migration::new("users".to_string(), 1, 2);
        assert_eq!(migration.status, MigrationStatus::Pending);

        migration.start();
        assert!(matches!(
            migration.status,
            MigrationStatus::InProgress { .. }
        ));

        migration.update_progress(0.5);
        if let MigrationStatus::InProgress { progress, .. } = migration.status {
            assert_eq!(progress, 0.5);
        } else {
            panic!("Expected InProgress status");
        }

        migration.complete(100);
        assert!(matches!(
            migration.status,
            MigrationStatus::Completed { .. }
        ));
    }
}
