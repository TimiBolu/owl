use std::collections::HashMap;
use thiserror::Error;

/// Context information for error reporting
#[derive(Debug, Clone)]
pub struct ErrorContext {
    pub operation: String,
    pub location: Option<String>,
    pub query_fragment: Option<String>,
    pub document_id: Option<String>,
    pub field_path: Option<String>,
    pub additional_info: HashMap<String, String>,
}

impl ErrorContext {
    pub fn new(operation: &str) -> Self {
        Self {
            operation: operation.to_string(),
            location: None,
            query_fragment: None,
            document_id: None,
            field_path: None,
            additional_info: HashMap::new(),
        }
    }

    pub fn with_location(mut self, location: &str) -> Self {
        self.location = Some(location.to_string());
        self
    }

    pub fn with_query_fragment(mut self, fragment: &str) -> Self {
        self.query_fragment = Some(fragment.to_string());
        self
    }

    pub fn with_document_id(mut self, id: &str) -> Self {
        self.document_id = Some(id.to_string());
        self
    }

    pub fn with_field_path(mut self, path: &str) -> Self {
        self.field_path = Some(path.to_string());
        self
    }

    pub fn with_info(mut self, key: &str, value: &str) -> Self {
        self.additional_info
            .insert(key.to_string(), value.to_string());
        self
    }
}

/// Severity levels for errors
#[derive(Debug, Clone, PartialEq)]
pub enum ErrorSeverity {
    Warning,  // Non-fatal, operation can continue
    Error,    // Fatal for current operation, but system can recover
    Critical, // System-level error requiring immediate attention
}

/// Recovery suggestions for common errors
#[derive(Debug, Clone)]
pub struct RecoverySuggestion {
    pub suggestion: String,
    pub example: Option<String>,
    pub auto_recoverable: bool,
}

/// Main error type for the OQL Database System
#[derive(Error, Debug)]
pub enum OQLError {
    #[error("Storage error: {message}")]
    StorageError { message: String },

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Document not found: {id}")]
    DocumentNotFound { id: String },

    #[error("Schema validation error: {message}")]
    SchemaValidation { message: String },

    #[error("Type error: Expected {expected}, got {actual}")]
    TypeError { expected: String, actual: String },

    #[error("Reference error: {message}")]
    ReferenceError { message: String },

    #[error("Parse error: {message}")]
    ParseError { message: String },

    #[error("Function error: {message}")]
    FunctionError { message: String },

    #[error("Transaction error: {message}")]
    TransactionError { message: String },

    #[error("Concurrency error: {message}")]
    ConcurrencyError { message: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Query execution error: {message}")]
    QueryExecution { message: String },

    #[error("Generic error: {message}")]
    Generic { message: String },

    // Enhanced error variants with full context
    #[error("Enhanced storage error: {message}")]
    EnhancedStorage {
        message: String,
        context: Option<ErrorContext>,
        severity: ErrorSeverity,
        recovery: Option<RecoverySuggestion>,
    },

    #[error("Enhanced serialization error: {message}")]
    EnhancedSerialization {
        message: String,
        context: Option<ErrorContext>,
        severity: ErrorSeverity,
        #[source]
        source: Option<serde_json::Error>,
    },

    #[error("Enhanced document not found: {id}")]
    EnhancedDocumentNotFound {
        id: String,
        context: Option<ErrorContext>,
        severity: ErrorSeverity,
        recovery: Option<RecoverySuggestion>,
    },

    #[error("Enhanced schema validation error: {message}")]
    EnhancedSchemaValidation {
        message: String,
        context: Option<ErrorContext>,
        severity: ErrorSeverity,
        recovery: Option<RecoverySuggestion>,
    },

    #[error("Enhanced type error: Expected {expected}, got {actual}")]
    EnhancedTypeError {
        expected: String,
        actual: String,
        context: Option<ErrorContext>,
        severity: ErrorSeverity,
        recovery: Option<RecoverySuggestion>,
    },

    #[error("Enhanced reference error: {message}")]
    EnhancedReferenceError {
        message: String,
        context: Option<ErrorContext>,
        severity: ErrorSeverity,
        recovery: Option<RecoverySuggestion>,
    },

    #[error("Enhanced parse error: {message}")]
    EnhancedParseError {
        message: String,
        context: Option<ErrorContext>,
        severity: ErrorSeverity,
        recovery: Option<RecoverySuggestion>,
    },

    #[error("Enhanced function error: {message}")]
    EnhancedFunctionError {
        message: String,
        context: Option<ErrorContext>,
        severity: ErrorSeverity,
        recovery: Option<RecoverySuggestion>,
    },

    #[error("Enhanced transaction error: {message}")]
    EnhancedTransactionError {
        message: String,
        context: Option<ErrorContext>,
        severity: ErrorSeverity,
        recovery: Option<RecoverySuggestion>,
    },

    #[error("Enhanced concurrency error: {message}")]
    EnhancedConcurrencyError {
        message: String,
        context: Option<ErrorContext>,
        severity: ErrorSeverity,
        recovery: Option<RecoverySuggestion>,
    },

    #[error("Enhanced IO error: {message}")]
    EnhancedIo {
        message: String,
        context: Option<ErrorContext>,
        severity: ErrorSeverity,
        #[source]
        source: Option<std::io::Error>,
    },

    #[error("Enhanced query execution error: {message}")]
    EnhancedQueryExecution {
        message: String,
        context: Option<ErrorContext>,
        severity: ErrorSeverity,
        recovery: Option<RecoverySuggestion>,
    },

    #[error("Enhanced generic error: {message}")]
    EnhancedGeneric {
        message: String,
        context: Option<ErrorContext>,
        severity: ErrorSeverity,
    },

    #[error("Batch operation error: {message}")]
    BatchError { message: String },

    #[error("IO error: {0}")]
    IoError(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Consistency error: {0}")]
    ConsistencyError(String),

    #[error("Recovery error: {0}")]
    RecoveryError(String),
}

impl OQLError {
    /// Get a user-friendly error message with context
    pub fn user_message(&self) -> String {
        let base_message = match self {
            // Legacy variants
            OQLError::StorageError { message } => format!("Storage error: {}", message),
            OQLError::Serialization(e) => format!("Data serialization error: {}", e),
            OQLError::DocumentNotFound { id } => format!("Document '{}' not found", id),
            OQLError::SchemaValidation { message } => {
                format!("Schema validation failed: {}", message)
            }
            OQLError::TypeError { expected, actual } => {
                format!("Type error: Expected {}, got {}", expected, actual)
            }
            OQLError::ReferenceError { message } => format!("Reference error: {}", message),
            OQLError::ParseError { message } => format!("Parse error: {}", message),
            OQLError::FunctionError { message } => format!("Function error: {}", message),
            OQLError::TransactionError { message } => format!("Transaction error: {}", message),
            OQLError::ConcurrencyError { message } => format!("Concurrency error: {}", message),
            OQLError::QueryExecution { message } => format!("Query execution error: {}", message),
            OQLError::Io(e) => format!("IO error: {}", e),
            OQLError::Generic { message } => message.clone(),

            // Enhanced variants
            OQLError::EnhancedStorage { message, .. } => format!("Storage error: {}", message),
            OQLError::EnhancedSerialization { message, .. } => {
                format!("Data serialization error: {}", message)
            }
            OQLError::EnhancedDocumentNotFound { id, .. } => format!("Document '{}' not found", id),
            OQLError::EnhancedSchemaValidation { message, .. } => {
                format!("Schema validation failed: {}", message)
            }
            OQLError::EnhancedTypeError {
                expected, actual, ..
            } => format!("Type error: Expected {}, got {}", expected, actual),
            OQLError::EnhancedReferenceError { message, .. } => {
                format!("Reference error: {}", message)
            }
            OQLError::EnhancedParseError { message, .. } => format!("Parse error: {}", message),
            OQLError::EnhancedFunctionError { message, .. } => {
                format!("Function error: {}", message)
            }
            OQLError::EnhancedTransactionError { message, .. } => {
                format!("Transaction error: {}", message)
            }
            OQLError::EnhancedConcurrencyError { message, .. } => {
                format!("Concurrency error: {}", message)
            }
            OQLError::EnhancedQueryExecution { message, .. } => {
                format!("Query execution error: {}", message)
            }
            OQLError::EnhancedIo { message, .. } => format!("IO error: {}", message),
            OQLError::BatchError { message } => format!("Batch operation error: {}", message),
            OQLError::EnhancedGeneric { message, .. } => message.clone(),
            OQLError::IoError(message) => format!("IO error: {}", message),
            OQLError::NotFound(message) => format!("Not found: {}", message),
            OQLError::ConsistencyError(message) => format!("Consistency error: {}", message),
            OQLError::RecoveryError(message) => format!("Recovery error: {}", message),
        };

        // Add context information if available
        if let Some(context) = self.context() {
            let mut parts = vec![base_message];

            if let Some(location) = &context.location {
                parts.push(format!("Location: {}", location));
            }

            if let Some(query) = &context.query_fragment {
                parts.push(format!("Query: {}", query));
            }

            if let Some(doc_id) = &context.document_id {
                parts.push(format!("Document: {}", doc_id));
            }

            if let Some(field) = &context.field_path {
                parts.push(format!("Field: {}", field));
            }

            parts.join(" | ")
        } else {
            base_message
        }
    }

    /// Get detailed error information for debugging
    pub fn detailed_message(&self) -> String {
        let mut parts = vec![self.user_message()];

        if let Some(recovery) = self.recovery_suggestion() {
            parts.push(format!("Suggestion: {}", recovery.suggestion));
            if let Some(example) = &recovery.example {
                parts.push(format!("Example: {}", example));
            }
        }

        if let Some(context) = self.context() {
            if !context.additional_info.is_empty() {
                let info: Vec<String> = context
                    .additional_info
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, v))
                    .collect();
                parts.push(format!("Additional info: {}", info.join(", ")));
            }
        }

        parts.join("\n")
    }

    /// Get error severity
    pub fn severity(&self) -> ErrorSeverity {
        match self {
            // Legacy variants - default severity
            OQLError::StorageError { .. } => ErrorSeverity::Error,
            OQLError::Serialization(_) => ErrorSeverity::Error,
            OQLError::DocumentNotFound { .. } => ErrorSeverity::Warning,
            OQLError::SchemaValidation { .. } => ErrorSeverity::Error,
            OQLError::TypeError { .. } => ErrorSeverity::Error,
            OQLError::ReferenceError { .. } => ErrorSeverity::Warning,
            OQLError::ParseError { .. } => ErrorSeverity::Error,
            OQLError::FunctionError { .. } => ErrorSeverity::Error,
            OQLError::TransactionError { .. } => ErrorSeverity::Error,
            OQLError::ConcurrencyError { .. } => ErrorSeverity::Warning,
            OQLError::QueryExecution { .. } => ErrorSeverity::Error,
            OQLError::Io(_) => ErrorSeverity::Error,
            OQLError::Generic { .. } => ErrorSeverity::Error,
            OQLError::BatchError { .. } => ErrorSeverity::Error,
            OQLError::IoError(_) => ErrorSeverity::Error,
            OQLError::NotFound(_) => ErrorSeverity::Warning,
            OQLError::ConsistencyError(_) => ErrorSeverity::Critical,
            OQLError::RecoveryError(_) => ErrorSeverity::Error,

            // Enhanced variants - explicit severity
            OQLError::EnhancedStorage { severity, .. } => severity.clone(),
            OQLError::EnhancedSerialization { severity, .. } => severity.clone(),
            OQLError::EnhancedDocumentNotFound { severity, .. } => severity.clone(),
            OQLError::EnhancedSchemaValidation { severity, .. } => severity.clone(),
            OQLError::EnhancedTypeError { severity, .. } => severity.clone(),
            OQLError::EnhancedReferenceError { severity, .. } => severity.clone(),
            OQLError::EnhancedParseError { severity, .. } => severity.clone(),
            OQLError::EnhancedFunctionError { severity, .. } => severity.clone(),
            OQLError::EnhancedTransactionError { severity, .. } => severity.clone(),
            OQLError::EnhancedConcurrencyError { severity, .. } => severity.clone(),
            OQLError::EnhancedQueryExecution { severity, .. } => severity.clone(),
            OQLError::EnhancedIo { severity, .. } => severity.clone(),
            OQLError::EnhancedGeneric { severity, .. } => severity.clone(),
        }
    }

    /// Get error context
    pub fn context(&self) -> Option<&ErrorContext> {
        match self {
            // Legacy variants - no context
            OQLError::StorageError { .. } => None,
            OQLError::Serialization(_) => None,
            OQLError::DocumentNotFound { .. } => None,
            OQLError::SchemaValidation { .. } => None,
            OQLError::TypeError { .. } => None,
            OQLError::ReferenceError { .. } => None,
            OQLError::ParseError { .. } => None,
            OQLError::FunctionError { .. } => None,
            OQLError::TransactionError { .. } => None,
            OQLError::ConcurrencyError { .. } => None,
            OQLError::QueryExecution { .. } => None,
            OQLError::Io(_) => None,
            OQLError::Generic { .. } => None,
            OQLError::BatchError { .. } => None,
            OQLError::IoError(_) => None,
            OQLError::NotFound(_) => None,
            OQLError::ConsistencyError(_) => None,
            OQLError::RecoveryError(_) => None,

            // Enhanced variants - with context
            OQLError::EnhancedStorage { context, .. } => context.as_ref(),
            OQLError::EnhancedSerialization { context, .. } => context.as_ref(),
            OQLError::EnhancedDocumentNotFound { context, .. } => context.as_ref(),
            OQLError::EnhancedSchemaValidation { context, .. } => context.as_ref(),
            OQLError::EnhancedTypeError { context, .. } => context.as_ref(),
            OQLError::EnhancedReferenceError { context, .. } => context.as_ref(),
            OQLError::EnhancedParseError { context, .. } => context.as_ref(),
            OQLError::EnhancedFunctionError { context, .. } => context.as_ref(),
            OQLError::EnhancedTransactionError { context, .. } => context.as_ref(),
            OQLError::EnhancedConcurrencyError { context, .. } => context.as_ref(),
            OQLError::EnhancedQueryExecution { context, .. } => context.as_ref(),
            OQLError::EnhancedIo { context, .. } => context.as_ref(),
            OQLError::EnhancedGeneric { context, .. } => context.as_ref(),
        }
    }

    /// Get recovery suggestion
    pub fn recovery_suggestion(&self) -> Option<&RecoverySuggestion> {
        match self {
            // Legacy variants - no recovery suggestions
            OQLError::StorageError { .. } => None,
            OQLError::Serialization(_) => None,
            OQLError::DocumentNotFound { .. } => None,
            OQLError::SchemaValidation { .. } => None,
            OQLError::TypeError { .. } => None,
            OQLError::ReferenceError { .. } => None,
            OQLError::ParseError { .. } => None,
            OQLError::FunctionError { .. } => None,
            OQLError::TransactionError { .. } => None,
            OQLError::ConcurrencyError { .. } => None,
            OQLError::QueryExecution { .. } => None,
            OQLError::Io(_) => None,
            OQLError::Generic { .. } => None,

            // Enhanced variants - with recovery suggestions
            OQLError::EnhancedStorage { recovery, .. } => recovery.as_ref(),
            OQLError::EnhancedDocumentNotFound { recovery, .. } => recovery.as_ref(),
            OQLError::EnhancedSchemaValidation { recovery, .. } => recovery.as_ref(),
            OQLError::EnhancedTypeError { recovery, .. } => recovery.as_ref(),
            OQLError::EnhancedReferenceError { recovery, .. } => recovery.as_ref(),
            OQLError::EnhancedParseError { recovery, .. } => recovery.as_ref(),
            OQLError::EnhancedFunctionError { recovery, .. } => recovery.as_ref(),
            OQLError::EnhancedTransactionError { recovery, .. } => recovery.as_ref(),
            OQLError::EnhancedConcurrencyError { recovery, .. } => recovery.as_ref(),
            OQLError::EnhancedQueryExecution { recovery, .. } => recovery.as_ref(),
            _ => None,
        }
    }

    /// Check if this error can be automatically recovered from
    pub fn is_auto_recoverable(&self) -> bool {
        self.recovery_suggestion()
            .map(|r| r.auto_recoverable)
            .unwrap_or(false)
    }

    /// Check if this is a warning (non-fatal error)
    pub fn is_warning(&self) -> bool {
        self.severity() == ErrorSeverity::Warning
    }

    /// Check if this is a critical error
    pub fn is_critical(&self) -> bool {
        self.severity() == ErrorSeverity::Critical
    }
}

impl Clone for OQLError {
    fn clone(&self) -> Self {
        match self {
            // Legacy variants - can be cloned except for those with source errors
            OQLError::StorageError { message } => OQLError::StorageError {
                message: message.clone(),
            },
            OQLError::Serialization(e) => OQLError::Generic {
                message: format!("Serialization error: {}", e),
            },
            OQLError::DocumentNotFound { id } => OQLError::DocumentNotFound { id: id.clone() },
            OQLError::SchemaValidation { message } => OQLError::SchemaValidation {
                message: message.clone(),
            },
            OQLError::TypeError { expected, actual } => OQLError::TypeError {
                expected: expected.clone(),
                actual: actual.clone(),
            },
            OQLError::ReferenceError { message } => OQLError::ReferenceError {
                message: message.clone(),
            },
            OQLError::ParseError { message } => OQLError::ParseError {
                message: message.clone(),
            },
            OQLError::FunctionError { message } => OQLError::FunctionError {
                message: message.clone(),
            },
            OQLError::TransactionError { message } => OQLError::TransactionError {
                message: message.clone(),
            },
            OQLError::ConcurrencyError { message } => OQLError::ConcurrencyError {
                message: message.clone(),
            },
            OQLError::QueryExecution { message } => OQLError::QueryExecution {
                message: message.clone(),
            },
            OQLError::Io(e) => OQLError::Generic {
                message: format!("IO error: {}", e),
            },
            OQLError::Generic { message } => OQLError::Generic {
                message: message.clone(),
            },
            OQLError::BatchError { message } => OQLError::BatchError {
                message: message.clone(),
            },
            OQLError::IoError(message) => OQLError::IoError(message.clone()),
            OQLError::NotFound(message) => OQLError::NotFound(message.clone()),
            OQLError::ConsistencyError(message) => OQLError::ConsistencyError(message.clone()),
            OQLError::RecoveryError(message) => OQLError::RecoveryError(message.clone()),

            // Enhanced variants - clone with context
            OQLError::EnhancedStorage {
                message,
                context,
                severity,
                recovery,
            } => OQLError::EnhancedStorage {
                message: message.clone(),
                context: context.clone(),
                severity: severity.clone(),
                recovery: recovery.clone(),
            },
            OQLError::EnhancedSerialization {
                message,
                context,
                severity,
                ..
            } => {
                OQLError::EnhancedSerialization {
                    message: message.clone(),
                    context: context.clone(),
                    severity: severity.clone(),
                    source: None, // Can't clone serde_json::Error
                }
            }
            OQLError::EnhancedDocumentNotFound {
                id,
                context,
                severity,
                recovery,
            } => OQLError::EnhancedDocumentNotFound {
                id: id.clone(),
                context: context.clone(),
                severity: severity.clone(),
                recovery: recovery.clone(),
            },
            OQLError::EnhancedSchemaValidation {
                message,
                context,
                severity,
                recovery,
            } => OQLError::EnhancedSchemaValidation {
                message: message.clone(),
                context: context.clone(),
                severity: severity.clone(),
                recovery: recovery.clone(),
            },
            OQLError::EnhancedTypeError {
                expected,
                actual,
                context,
                severity,
                recovery,
            } => OQLError::EnhancedTypeError {
                expected: expected.clone(),
                actual: actual.clone(),
                context: context.clone(),
                severity: severity.clone(),
                recovery: recovery.clone(),
            },
            OQLError::EnhancedReferenceError {
                message,
                context,
                severity,
                recovery,
            } => OQLError::EnhancedReferenceError {
                message: message.clone(),
                context: context.clone(),
                severity: severity.clone(),
                recovery: recovery.clone(),
            },
            OQLError::EnhancedParseError {
                message,
                context,
                severity,
                recovery,
            } => OQLError::EnhancedParseError {
                message: message.clone(),
                context: context.clone(),
                severity: severity.clone(),
                recovery: recovery.clone(),
            },
            OQLError::EnhancedFunctionError {
                message,
                context,
                severity,
                recovery,
            } => OQLError::EnhancedFunctionError {
                message: message.clone(),
                context: context.clone(),
                severity: severity.clone(),
                recovery: recovery.clone(),
            },
            OQLError::EnhancedTransactionError {
                message,
                context,
                severity,
                recovery,
            } => OQLError::EnhancedTransactionError {
                message: message.clone(),
                context: context.clone(),
                severity: severity.clone(),
                recovery: recovery.clone(),
            },
            OQLError::EnhancedConcurrencyError {
                message,
                context,
                severity,
                recovery,
            } => OQLError::EnhancedConcurrencyError {
                message: message.clone(),
                context: context.clone(),
                severity: severity.clone(),
                recovery: recovery.clone(),
            },
            OQLError::EnhancedIo {
                message,
                context,
                severity,
                ..
            } => {
                OQLError::EnhancedIo {
                    message: message.clone(),
                    context: context.clone(),
                    severity: severity.clone(),
                    source: None, // Can't clone the source error
                }
            }
            OQLError::EnhancedQueryExecution {
                message,
                context,
                severity,
                recovery,
            } => OQLError::EnhancedQueryExecution {
                message: message.clone(),
                context: context.clone(),
                severity: severity.clone(),
                recovery: recovery.clone(),
            },
            OQLError::EnhancedGeneric {
                message,
                context,
                severity,
            } => OQLError::EnhancedGeneric {
                message: message.clone(),
                context: context.clone(),
                severity: severity.clone(),
            },
        }
    }
}

/// Convenience constructors for enhanced error types
impl OQLError {
    /// Create a storage error with context
    pub fn storage_error(message: &str, context: Option<ErrorContext>) -> Self {
        OQLError::EnhancedStorage {
            message: message.to_string(),
            context,
            severity: ErrorSeverity::Error,
            recovery: None,
        }
    }

    /// Create a document not found error with recovery suggestion
    pub fn document_not_found(id: &str, context: Option<ErrorContext>) -> Self {
        let recovery = RecoverySuggestion {
            suggestion: "Check if the document ID is correct or if the document exists".to_string(),
            example: Some(format!(
                "Use: {}.exists to check if document exists first",
                id
            )),
            auto_recoverable: false,
        };

        OQLError::EnhancedDocumentNotFound {
            id: id.to_string(),
            context,
            severity: ErrorSeverity::Warning,
            recovery: Some(recovery),
        }
    }

    /// Create a type error with recovery suggestion
    pub fn type_error(expected: &str, actual: &str, context: Option<ErrorContext>) -> Self {
        let recovery = RecoverySuggestion {
            suggestion: format!("Convert the value to {} or use a compatible type", expected),
            example: Some(format!(
                "Use type conversion functions or check field types"
            )),
            auto_recoverable: false,
        };

        OQLError::EnhancedTypeError {
            expected: expected.to_string(),
            actual: actual.to_string(),
            context,
            severity: ErrorSeverity::Error,
            recovery: Some(recovery),
        }
    }

    /// Create a reference error with recovery suggestion
    pub fn reference_error(message: &str, context: Option<ErrorContext>) -> Self {
        let recovery = RecoverySuggestion {
            suggestion: "Check if the referenced document exists and the reference path is correct"
                .to_string(),
            example: Some("Use .exists to verify references before navigation".to_string()),
            auto_recoverable: false,
        };

        OQLError::EnhancedReferenceError {
            message: message.to_string(),
            context,
            severity: ErrorSeverity::Warning,
            recovery: Some(recovery),
        }
    }

    /// Create a parse error with recovery suggestion
    pub fn parse_error(message: &str, context: Option<ErrorContext>) -> Self {
        let recovery = RecoverySuggestion {
            suggestion: "Check query syntax and ensure all brackets and operators are correct"
                .to_string(),
            example: Some("Example: collection.where { .field == \"value\" }".to_string()),
            auto_recoverable: false,
        };

        OQLError::EnhancedParseError {
            message: message.to_string(),
            context,
            severity: ErrorSeverity::Error,
            recovery: Some(recovery),
        }
    }

    /// Create a function error with recovery suggestion
    pub fn function_error(message: &str, context: Option<ErrorContext>) -> Self {
        let recovery = RecoverySuggestion {
            suggestion: "Check function name, argument count, and argument types".to_string(),
            example: Some(
                "Use built-in functions like count(), sum(), avg() or define custom functions"
                    .to_string(),
            ),
            auto_recoverable: false,
        };

        OQLError::EnhancedFunctionError {
            message: message.to_string(),
            context,
            severity: ErrorSeverity::Error,
            recovery: Some(recovery),
        }
    }

    /// Create a transaction error
    pub fn transaction_error(message: &str, context: Option<ErrorContext>) -> Self {
        let recovery = RecoverySuggestion {
            suggestion: "Retry the transaction or check for conflicting operations".to_string(),
            example: Some("Use transaction { ... } blocks for atomic operations".to_string()),
            auto_recoverable: true,
        };

        OQLError::EnhancedTransactionError {
            message: message.to_string(),
            context,
            severity: ErrorSeverity::Error,
            recovery: Some(recovery),
        }
    }

    /// Create a concurrency error
    pub fn concurrency_error(message: &str, context: Option<ErrorContext>) -> Self {
        let recovery = RecoverySuggestion {
            suggestion: "Retry the operation after a brief delay".to_string(),
            example: Some("The system will automatically retry conflicting operations".to_string()),
            auto_recoverable: true,
        };

        OQLError::EnhancedConcurrencyError {
            message: message.to_string(),
            context,
            severity: ErrorSeverity::Warning,
            recovery: Some(recovery),
        }
    }

    /// Create a generic error
    pub fn generic(message: &str) -> Self {
        OQLError::Generic {
            message: message.to_string(),
        }
    }

    /// Create a serialization error with context
    pub fn serialization_error(message: &str, context: Option<ErrorContext>) -> Self {
        OQLError::EnhancedSerialization {
            message: message.to_string(),
            context,
            severity: ErrorSeverity::Error,
            source: None,
        }
    }
}

// Note: From implementations are already handled by the #[from] attributes in the enum

/// Partial result with error annotations
#[derive(Debug, Clone)]
pub struct PartialResult<T> {
    pub data: T,
    pub errors: Vec<OQLError>,
    pub warnings: Vec<OQLError>,
}

impl<T> PartialResult<T> {
    /// Create a new partial result with data
    pub fn new(data: T) -> Self {
        Self {
            data,
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Create a partial result with data and errors
    pub fn with_errors(data: T, errors: Vec<OQLError>) -> Self {
        let mut warnings = Vec::new();
        let mut actual_errors = Vec::new();

        for error in errors {
            if error.is_warning() {
                warnings.push(error);
            } else {
                actual_errors.push(error);
            }
        }

        Self {
            data,
            errors: actual_errors,
            warnings,
        }
    }

    /// Add an error to the result
    pub fn add_error(&mut self, error: OQLError) {
        if error.is_warning() {
            self.warnings.push(error);
        } else {
            self.errors.push(error);
        }
    }

    /// Check if there are any errors (not warnings)
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Check if there are any warnings
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }

    /// Get all issues (errors and warnings)
    pub fn all_issues(&self) -> Vec<&OQLError> {
        self.errors.iter().chain(self.warnings.iter()).collect()
    }

    /// Convert to a regular Result if there are errors
    pub fn into_result(self) -> Result<T, Vec<OQLError>> {
        if self.has_errors() {
            Err(self.errors)
        } else {
            Ok(self.data)
        }
    }
}

/// Error recovery utilities
pub struct ErrorRecovery;

impl ErrorRecovery {
    /// Attempt to recover from an error automatically
    pub fn attempt_recovery(error: &OQLError) -> Option<String> {
        if !error.is_auto_recoverable() {
            return None;
        }

        match error {
            OQLError::ConcurrencyError { .. } => {
                Some("Retrying operation after brief delay...".to_string())
            }
            OQLError::TransactionError { .. } => {
                Some("Attempting transaction retry...".to_string())
            }
            OQLError::EnhancedConcurrencyError { .. } => {
                Some("Retrying operation after brief delay...".to_string())
            }
            OQLError::EnhancedTransactionError { .. } => {
                Some("Attempting transaction retry...".to_string())
            }
            _ => None,
        }
    }

    /// Get suggested fixes for common errors
    pub fn suggest_fix(error: &OQLError) -> Vec<String> {
        let mut suggestions = Vec::new();

        if let Some(recovery) = error.recovery_suggestion() {
            suggestions.push(recovery.suggestion.clone());
            if let Some(example) = &recovery.example {
                suggestions.push(format!("Example: {}", example));
            }
        }

        // Add context-specific suggestions
        if let Some(context) = error.context() {
            match error {
                OQLError::ReferenceError { .. } | OQLError::EnhancedReferenceError { .. } => {
                    if let Some(field_path) = &context.field_path {
                        suggestions.push(format!(
                            "Check if the reference path '{}' is correct",
                            field_path
                        ));
                    }
                }
                OQLError::TypeError { .. } | OQLError::EnhancedTypeError { .. } => {
                    if let Some(field_path) = &context.field_path {
                        suggestions.push(format!("Verify the type of field '{}'", field_path));
                    }
                }
                _ => {}
            }
        }

        suggestions
    }
}

/// Result type alias for OQL operations
pub type OQLResult<T> = Result<T, OQLError>;

/// Result type for operations that can return partial results
pub type OQLPartialResult<T> = Result<PartialResult<T>, OQLError>;
