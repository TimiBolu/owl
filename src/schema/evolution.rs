use crate::error::{OQLError, OQLResult};
use crate::schema::{FieldType, Schema};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Schema version with metadata
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SchemaVersion {
    pub schema_name: String,
    pub version: u32,
    pub schema: Schema,
    pub created_at: DateTime<Utc>,
    pub description: String,
    pub changes: Vec<FieldChange>,
}

impl SchemaVersion {
    /// Create a new schema version
    pub fn new(schema: Schema, version: u32, description: String) -> Self {
        Self {
            schema_name: schema.name.clone(),
            version,
            schema,
            created_at: Utc::now(),
            description,
            changes: Vec::new(),
        }
    }

    /// Create initial version (v1)
    pub fn initial(schema: Schema) -> Self {
        Self::new(schema, 1, "Initial schema version".to_string())
    }

    /// Create next version with changes
    pub fn next(&self, new_schema: Schema, description: String, changes: Vec<FieldChange>) -> Self {
        Self {
            schema_name: new_schema.name.clone(),
            version: self.version + 1,
            schema: new_schema,
            created_at: Utc::now(),
            description,
            changes,
        }
    }
}

/// Types of field changes in schema evolution
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FieldChange {
    /// Add a new field
    AddField {
        field_name: String,
        field_type: FieldType,
        default_value: Option<String>,
    },
    /// Remove an existing field
    RemoveField { field_name: String },
    /// Rename a field
    RenameField { old_name: String, new_name: String },
    /// Change field type
    ChangeType {
        field_name: String,
        old_type: FieldType,
        new_type: FieldType,
    },
    /// Make field optional
    MakeOptional { field_name: String },
    /// Make field required
    MakeRequired {
        field_name: String,
        default_value: Option<String>,
    },
    /// Change reference target
    ChangeReferenceTarget {
        field_name: String,
        old_target: String,
        new_target: String,
    },
}

impl FieldChange {
    /// Get a human-readable description of the change
    pub fn description(&self) -> String {
        match self {
            FieldChange::AddField {
                field_name,
                field_type,
                ..
            } => {
                format!(
                    "Add field '{}' of type {}",
                    field_name,
                    field_type.type_name()
                )
            }
            FieldChange::RemoveField { field_name } => {
                format!("Remove field '{}'", field_name)
            }
            FieldChange::RenameField { old_name, new_name } => {
                format!("Rename field '{}' to '{}'", old_name, new_name)
            }
            FieldChange::ChangeType {
                field_name,
                old_type,
                new_type,
            } => {
                format!(
                    "Change field '{}' type from {} to {}",
                    field_name,
                    old_type.type_name(),
                    new_type.type_name()
                )
            }
            FieldChange::MakeOptional { field_name } => {
                format!("Make field '{}' optional", field_name)
            }
            FieldChange::MakeRequired { field_name, .. } => {
                format!("Make field '{}' required", field_name)
            }
            FieldChange::ChangeReferenceTarget {
                field_name,
                old_target,
                new_target,
            } => {
                format!(
                    "Change reference '{}' from {} to {}",
                    field_name, old_target, new_target
                )
            }
        }
    }
}

/// Compatibility level for schema changes
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompatibilityLevel {
    /// Fully backward compatible - old data works with new schema
    FullyCompatible,
    /// Backward compatible with migration - old data can be migrated
    BackwardCompatible,
    /// Breaking change - requires data migration and may lose data
    Breaking,
}

/// Result of compatibility check
#[derive(Debug, Clone)]
pub struct CompatibilityCheck {
    pub level: CompatibilityLevel,
    pub issues: Vec<String>,
    pub warnings: Vec<String>,
    pub migration_required: bool,
}

impl CompatibilityCheck {
    /// Create a fully compatible result
    pub fn compatible() -> Self {
        Self {
            level: CompatibilityLevel::FullyCompatible,
            issues: Vec::new(),
            warnings: Vec::new(),
            migration_required: false,
        }
    }

    /// Create a backward compatible result
    pub fn backward_compatible(warnings: Vec<String>) -> Self {
        Self {
            level: CompatibilityLevel::BackwardCompatible,
            issues: Vec::new(),
            warnings,
            migration_required: true,
        }
    }

    /// Create a breaking change result
    pub fn breaking(issues: Vec<String>) -> Self {
        Self {
            level: CompatibilityLevel::Breaking,
            issues,
            warnings: Vec::new(),
            migration_required: true,
        }
    }

    /// Check if the change is safe
    pub fn is_safe(&self) -> bool {
        self.level != CompatibilityLevel::Breaking
    }
}

/// Schema evolution manager
pub struct SchemaEvolution {
    versions: HashMap<String, Vec<SchemaVersion>>,
}

impl SchemaEvolution {
    /// Create a new schema evolution manager
    pub fn new() -> Self {
        Self {
            versions: HashMap::new(),
        }
    }

    /// Register a schema version
    pub fn register_version(&mut self, version: SchemaVersion) -> OQLResult<()> {
        let schema_name = version.schema_name.clone();
        let versions = self
            .versions
            .entry(schema_name.clone())
            .or_insert_with(Vec::new);

        // Check for duplicate version numbers
        if versions.iter().any(|v| v.version == version.version) {
            return Err(OQLError::SchemaValidation {
                message: format!(
                    "Version {} already exists for schema '{}'",
                    version.version, schema_name
                ),
            });
        }

        versions.push(version);
        versions.sort_by_key(|v| v.version);

        Ok(())
    }

    /// Get the latest version of a schema
    pub fn get_latest(&self, schema_name: &str) -> Option<&SchemaVersion> {
        self.versions.get(schema_name)?.last()
    }

    /// Get a specific version of a schema
    pub fn get_version(&self, schema_name: &str, version: u32) -> Option<&SchemaVersion> {
        self.versions
            .get(schema_name)?
            .iter()
            .find(|v| v.version == version)
    }

    /// Get all versions of a schema
    pub fn get_all_versions(&self, schema_name: &str) -> Vec<&SchemaVersion> {
        self.versions
            .get(schema_name)
            .map(|versions| versions.iter().collect())
            .unwrap_or_default()
    }

    /// Detect changes between two schemas
    pub fn detect_changes(&self, old_schema: &Schema, new_schema: &Schema) -> Vec<FieldChange> {
        let mut changes = Vec::new();

        // Check for removed fields
        for (field_name, _) in &old_schema.fields {
            if !new_schema.fields.contains_key(field_name) {
                changes.push(FieldChange::RemoveField {
                    field_name: field_name.clone(),
                });
            }
        }

        // Check for added and modified fields
        for (field_name, new_type) in &new_schema.fields {
            if let Some(old_type) = old_schema.fields.get(field_name) {
                // Field exists in both - check for type changes
                if old_type != new_type {
                    // Check if it's just making a field optional
                    if Self::is_make_optional(old_type, new_type) {
                        changes.push(FieldChange::MakeOptional {
                            field_name: field_name.clone(),
                        });
                    } else if Self::is_make_required(old_type, new_type) {
                        changes.push(FieldChange::MakeRequired {
                            field_name: field_name.clone(),
                            default_value: None,
                        });
                    } else if Self::is_reference_target_change(old_type, new_type) {
                        if let (Some(old_target), Some(new_target)) =
                            (old_type.referenced_schema(), new_type.referenced_schema())
                        {
                            changes.push(FieldChange::ChangeReferenceTarget {
                                field_name: field_name.clone(),
                                old_target: old_target.to_string(),
                                new_target: new_target.to_string(),
                            });
                        }
                    } else {
                        changes.push(FieldChange::ChangeType {
                            field_name: field_name.clone(),
                            old_type: old_type.clone(),
                            new_type: new_type.clone(),
                        });
                    }
                }
            } else {
                // New field
                changes.push(FieldChange::AddField {
                    field_name: field_name.clone(),
                    field_type: new_type.clone(),
                    default_value: None,
                });
            }
        }

        changes
    }

    /// Check if a type change is making a field optional
    fn is_make_optional(old_type: &FieldType, new_type: &FieldType) -> bool {
        match (old_type, new_type) {
            (FieldType::Optional(_), _) => false,
            (_, FieldType::Optional(inner)) => {
                // Check if the inner type matches the old type
                **inner == *old_type
            }
            _ => false,
        }
    }

    /// Check if a type change is making a field required
    fn is_make_required(old_type: &FieldType, new_type: &FieldType) -> bool {
        match (old_type, new_type) {
            (FieldType::Optional(inner), _) => **inner == *new_type,
            _ => false,
        }
    }

    /// Check if a type change is just changing reference target
    fn is_reference_target_change(old_type: &FieldType, new_type: &FieldType) -> bool {
        match (old_type, new_type) {
            (FieldType::Reference(_), FieldType::Reference(_)) => true,
            _ => false,
        }
    }

    /// Check compatibility between two schema versions
    pub fn check_compatibility(
        &self,
        old_schema: &Schema,
        new_schema: &Schema,
    ) -> CompatibilityCheck {
        let changes = self.detect_changes(old_schema, new_schema);

        if changes.is_empty() {
            return CompatibilityCheck::compatible();
        }

        let mut issues = Vec::new();
        let mut warnings = Vec::new();
        let mut is_breaking = false;

        for change in &changes {
            match change {
                FieldChange::AddField {
                    field_name,
                    field_type,
                    ..
                } => {
                    if !field_type.is_optional() {
                        issues.push(format!(
                            "Adding required field '{}' is a breaking change - existing documents won't have this field",
                            field_name
                        ));
                        is_breaking = true;
                    } else {
                        warnings.push(format!(
                            "Adding optional field '{}' - existing documents will have null values",
                            field_name
                        ));
                    }
                }
                FieldChange::RemoveField { field_name } => {
                    issues.push(format!(
                        "Removing field '{}' is a breaking change - data will be lost",
                        field_name
                    ));
                    is_breaking = true;
                }
                FieldChange::RenameField { old_name, new_name } => {
                    warnings.push(format!(
                        "Renaming field '{}' to '{}' requires data migration",
                        old_name, new_name
                    ));
                }
                FieldChange::ChangeType {
                    field_name,
                    old_type,
                    new_type,
                } => {
                    if Self::is_compatible_type_change(old_type, new_type) {
                        warnings.push(format!(
                            "Changing type of '{}' from {} to {} may require data conversion",
                            field_name,
                            old_type.type_name(),
                            new_type.type_name()
                        ));
                    } else {
                        issues.push(format!(
                            "Changing type of '{}' from {} to {} is incompatible",
                            field_name,
                            old_type.type_name(),
                            new_type.type_name()
                        ));
                        is_breaking = true;
                    }
                }
                FieldChange::MakeOptional { field_name } => {
                    warnings.push(format!(
                        "Making field '{}' optional is backward compatible",
                        field_name
                    ));
                }
                FieldChange::MakeRequired { field_name, .. } => {
                    issues.push(format!(
                        "Making field '{}' required is a breaking change - existing documents may have null values",
                        field_name
                    ));
                    is_breaking = true;
                }
                FieldChange::ChangeReferenceTarget {
                    field_name,
                    old_target,
                    new_target,
                } => {
                    issues.push(format!(
                        "Changing reference target of '{}' from {} to {} is a breaking change",
                        field_name, old_target, new_target
                    ));
                    is_breaking = true;
                }
            }
        }

        if is_breaking {
            CompatibilityCheck::breaking(issues)
        } else if !warnings.is_empty() {
            CompatibilityCheck::backward_compatible(warnings)
        } else {
            CompatibilityCheck::compatible()
        }
    }

    /// Check if a type change is compatible (can be converted)
    fn is_compatible_type_change(old_type: &FieldType, new_type: &FieldType) -> bool {
        match (old_type, new_type) {
            // Integer to Float is compatible
            (FieldType::Integer, FieldType::Float) => true,
            // String conversions are generally compatible
            (_, FieldType::String) => true,
            // Same type is compatible
            (a, b) if a == b => true,
            _ => false,
        }
    }

    /// Validate a schema evolution
    pub fn validate_evolution(
        &self,
        old_version: &SchemaVersion,
        new_version: &SchemaVersion,
    ) -> OQLResult<()> {
        // Check version numbers
        if new_version.version <= old_version.version {
            return Err(OQLError::SchemaValidation {
                message: format!(
                    "New version {} must be greater than old version {}",
                    new_version.version, old_version.version
                ),
            });
        }

        // Check schema names match
        if old_version.schema_name != new_version.schema_name {
            return Err(OQLError::SchemaValidation {
                message: format!(
                    "Schema names don't match: '{}' vs '{}'",
                    old_version.schema_name, new_version.schema_name
                ),
            });
        }

        // Check compatibility
        let compat = self.check_compatibility(&old_version.schema, &new_version.schema);
        if !compat.is_safe() {
            return Err(OQLError::SchemaValidation {
                message: format!("Breaking changes detected: {}", compat.issues.join(", ")),
            });
        }

        Ok(())
    }
}

impl Default for SchemaEvolution {
    fn default() -> Self {
        Self::new()
    }
}

/// Custom error type for evolution-specific errors
#[derive(Debug, Clone)]
pub struct EvolutionError {
    pub message: String,
    pub compatibility_issues: Vec<String>,
}

impl EvolutionError {
    pub fn new(message: String) -> Self {
        Self {
            message,
            compatibility_issues: Vec::new(),
        }
    }

    pub fn with_issues(message: String, issues: Vec<String>) -> Self {
        Self {
            message,
            compatibility_issues: issues,
        }
    }
}

impl std::fmt::Display for EvolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)?;
        if !self.compatibility_issues.is_empty() {
            write!(f, "\nIssues:\n")?;
            for issue in &self.compatibility_issues {
                write!(f, "  - {}\n", issue)?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for EvolutionError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn create_test_schema(name: &str) -> Schema {
        let mut fields = HashMap::new();
        fields.insert("id".to_string(), FieldType::String);
        fields.insert("name".to_string(), FieldType::String);
        Schema::new(name.to_string(), fields)
    }

    #[test]
    fn test_schema_version_creation() {
        let schema = create_test_schema("users");
        let version = SchemaVersion::initial(schema.clone());

        assert_eq!(version.version, 1);
        assert_eq!(version.schema_name, "users");
        assert_eq!(version.description, "Initial schema version");
    }

    #[test]
    fn test_detect_add_field() {
        let mut old_fields = HashMap::new();
        old_fields.insert("name".to_string(), FieldType::String);
        let old_schema = Schema::new("users".to_string(), old_fields);

        let mut new_fields = HashMap::new();
        new_fields.insert("name".to_string(), FieldType::String);
        new_fields.insert("email".to_string(), FieldType::String);
        let new_schema = Schema::new("users".to_string(), new_fields);

        let evolution = SchemaEvolution::new();
        let changes = evolution.detect_changes(&old_schema, &new_schema);

        assert_eq!(changes.len(), 1);
        match &changes[0] {
            FieldChange::AddField { field_name, .. } => {
                assert_eq!(field_name, "email");
            }
            _ => panic!("Expected AddField change"),
        }
    }

    #[test]
    fn test_detect_remove_field() {
        let mut old_fields = HashMap::new();
        old_fields.insert("name".to_string(), FieldType::String);
        old_fields.insert("email".to_string(), FieldType::String);
        let old_schema = Schema::new("users".to_string(), old_fields);

        let mut new_fields = HashMap::new();
        new_fields.insert("name".to_string(), FieldType::String);
        let new_schema = Schema::new("users".to_string(), new_fields);

        let evolution = SchemaEvolution::new();
        let changes = evolution.detect_changes(&old_schema, &new_schema);

        assert_eq!(changes.len(), 1);
        match &changes[0] {
            FieldChange::RemoveField { field_name } => {
                assert_eq!(field_name, "email");
            }
            _ => panic!("Expected RemoveField change"),
        }
    }

    #[test]
    fn test_detect_make_optional() {
        let mut old_fields = HashMap::new();
        old_fields.insert("email".to_string(), FieldType::String);
        let old_schema = Schema::new("users".to_string(), old_fields);

        let mut new_fields = HashMap::new();
        new_fields.insert(
            "email".to_string(),
            FieldType::Optional(Box::new(FieldType::String)),
        );
        let new_schema = Schema::new("users".to_string(), new_fields);

        let evolution = SchemaEvolution::new();
        let changes = evolution.detect_changes(&old_schema, &new_schema);

        assert_eq!(changes.len(), 1);
        match &changes[0] {
            FieldChange::MakeOptional { field_name } => {
                assert_eq!(field_name, "email");
            }
            _ => panic!("Expected MakeOptional change"),
        }
    }

    #[test]
    fn test_compatibility_add_optional_field() {
        let mut old_fields = HashMap::new();
        old_fields.insert("name".to_string(), FieldType::String);
        let old_schema = Schema::new("users".to_string(), old_fields);

        let mut new_fields = HashMap::new();
        new_fields.insert("name".to_string(), FieldType::String);
        new_fields.insert(
            "email".to_string(),
            FieldType::Optional(Box::new(FieldType::String)),
        );
        let new_schema = Schema::new("users".to_string(), new_fields);

        let evolution = SchemaEvolution::new();
        let compat = evolution.check_compatibility(&old_schema, &new_schema);

        assert_eq!(compat.level, CompatibilityLevel::BackwardCompatible);
        assert!(compat.is_safe());
    }

    #[test]
    fn test_compatibility_add_required_field() {
        let mut old_fields = HashMap::new();
        old_fields.insert("name".to_string(), FieldType::String);
        let old_schema = Schema::new("users".to_string(), old_fields);

        let mut new_fields = HashMap::new();
        new_fields.insert("name".to_string(), FieldType::String);
        new_fields.insert("email".to_string(), FieldType::String);
        let new_schema = Schema::new("users".to_string(), new_fields);

        let evolution = SchemaEvolution::new();
        let compat = evolution.check_compatibility(&old_schema, &new_schema);

        assert_eq!(compat.level, CompatibilityLevel::Breaking);
        assert!(!compat.is_safe());
    }

    #[test]
    fn test_compatibility_remove_field() {
        let mut old_fields = HashMap::new();
        old_fields.insert("name".to_string(), FieldType::String);
        old_fields.insert("email".to_string(), FieldType::String);
        let old_schema = Schema::new("users".to_string(), old_fields);

        let mut new_fields = HashMap::new();
        new_fields.insert("name".to_string(), FieldType::String);
        let new_schema = Schema::new("users".to_string(), new_fields);

        let evolution = SchemaEvolution::new();
        let compat = evolution.check_compatibility(&old_schema, &new_schema);

        assert_eq!(compat.level, CompatibilityLevel::Breaking);
        assert!(!compat.is_safe());
    }
}
