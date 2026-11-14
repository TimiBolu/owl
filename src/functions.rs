// Built-in Function Framework for OQL
// Provides string manipulation, math, and date/time functions

use crate::document::Value;
use crate::error::{OQLError, OQLResult};
use crate::parser::FunctionDefinition;
use chrono::{DateTime, Datelike, Duration, NaiveDateTime, TimeZone, Timelike, Utc};
use std::collections::HashMap;

/// Function signature for built-in functions
pub type BuiltinFunction = fn(&[Value]) -> OQLResult<Value>;

/// User-defined function with its definition and parameter types
#[derive(Debug, Clone)]
pub struct UserDefinedFunction {
    pub definition: FunctionDefinition,
    pub parameter_types: Vec<String>,
    pub return_type: Option<String>,
}

/// Registry of all functions (built-in and user-defined)
pub struct FunctionRegistry {
    builtin_functions: HashMap<String, BuiltinFunction>,
    user_functions: HashMap<String, UserDefinedFunction>,
}

impl FunctionRegistry {
    /// Create a new function registry with all built-in functions
    pub fn new() -> Self {
        let mut registry = Self {
            builtin_functions: HashMap::new(),
            user_functions: HashMap::new(),
        };

        // Register all built-in functions
        registry.register_string_functions();
        registry.register_math_functions();
        registry.register_datetime_functions();
        registry.register_utility_functions();

        registry
    }

    /// Get a built-in function by name
    pub fn get_function(&self, name: &str) -> Option<&BuiltinFunction> {
        self.builtin_functions.get(name)
    }

    /// Get a user-defined function by name
    pub fn get_user_function(&self, name: &str) -> Option<&UserDefinedFunction> {
        self.user_functions.get(name)
    }

    /// Check if a function exists (built-in or user-defined)
    pub fn has_function(&self, name: &str) -> bool {
        self.builtin_functions.contains_key(name) || self.user_functions.contains_key(name)
    }

    /// Get all function names
    pub fn function_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        names.extend(self.builtin_functions.keys().cloned());
        names.extend(self.user_functions.keys().cloned());
        names
    }

    /// Register a built-in function
    fn register(&mut self, name: &str, func: BuiltinFunction) {
        self.builtin_functions.insert(name.to_string(), func);
    }

    /// Register a user-defined function
    pub fn register_user_function(&mut self, func_def: FunctionDefinition) -> OQLResult<()> {
        // Validate function definition
        self.validate_function_definition(&func_def)?;

        let parameter_types = func_def
            .parameters
            .iter()
            .map(|p| p.param_type.clone())
            .collect();

        let user_func = UserDefinedFunction {
            return_type: func_def.return_type.clone(),
            parameter_types,
            definition: func_def.clone(),
        };

        self.user_functions.insert(func_def.name.clone(), user_func);
        Ok(())
    }

    /// Validate a function definition
    fn validate_function_definition(&self, func_def: &FunctionDefinition) -> OQLResult<()> {
        // Check for duplicate function names
        if self.has_function(&func_def.name) {
            return Err(OQLError::Generic {
                message: format!("Function '{}' already exists", func_def.name),
            });
        }

        // Validate parameter types
        for param in &func_def.parameters {
            if !self.is_valid_type(&param.param_type) {
                return Err(OQLError::TypeError {
                    expected: "valid type".to_string(),
                    actual: param.param_type.clone(),
                });
            }
        }

        // Validate return type if specified
        if let Some(return_type) = &func_def.return_type {
            if !self.is_valid_type(return_type) {
                return Err(OQLError::TypeError {
                    expected: "valid type".to_string(),
                    actual: return_type.to_string(),
                });
            }
        }

        Ok(())
    }

    /// Check if a type name is valid
    fn is_valid_type(&self, type_name: &str) -> bool {
        let trimmed = type_name.trim();
        if trimmed.is_empty() {
            return false;
        }
        if matches!(
            trimmed.to_ascii_lowercase().as_str(),
            "string"
                | "integer"
                | "int"
                | "float"
                | "double"
                | "boolean"
                | "bool"
                | "datetime"
                | "timestamp"
        ) {
            return true;
        }

        // Treat identifier-like names as schema/object types.
        // This keeps function definitions flexible with evolving schemas.
        let mut chars = trimmed.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        (first.is_ascii_alphabetic() || first == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    /// Register all string manipulation functions
    fn register_string_functions(&mut self) {
        self.register("trim", string_trim);
        self.register("upper", string_upper);
        self.register("lower", string_lower);
        self.register("length", string_length);
        self.register("substring", string_substring);
        self.register("concat", string_concat);
        self.register("replace", string_replace);
        self.register("split", string_split);
        self.register("starts_with", string_starts_with);
        self.register("ends_with", string_ends_with);
        self.register("contains", string_contains);
        self.register("pad_left", string_pad_left);
        self.register("pad_right", string_pad_right);
    }

    /// Register all math functions
    fn register_math_functions(&mut self) {
        self.register("abs", math_abs);
        self.register("round", math_round);
        self.register("floor", math_floor);
        self.register("ceil", math_ceil);
        self.register("sqrt", math_sqrt);
        self.register("pow", math_pow);
        self.register("min", math_min);
        self.register("max", math_max);
        self.register("sin", math_sin);
        self.register("cos", math_cos);
        self.register("tan", math_tan);
        self.register("log", math_log);
        self.register("ln", math_ln);
        self.register("exp", math_exp);
        self.register("random", math_random);
    }

    /// Register all date/time functions
    fn register_datetime_functions(&mut self) {
        self.register("now", datetime_now);
        self.register("today", datetime_today);
        self.register("days_ago", datetime_days_ago);
        self.register("hours_ago", datetime_hours_ago);
        self.register("minutes_ago", datetime_minutes_ago);
        self.register("format_date", datetime_format);
        self.register("parse_date", datetime_parse);
        self.register("year", datetime_year);
        self.register("month", datetime_month);
        self.register("day", datetime_day);
        self.register("hour", datetime_hour);
        self.register("minute", datetime_minute);
        self.register("second", datetime_second);
        self.register("weekday", datetime_weekday);
        self.register("add_days", datetime_add_days);
        self.register("add_hours", datetime_add_hours);
        self.register("diff_days", datetime_diff_days);
    }

    /// Register utility functions
    fn register_utility_functions(&mut self) {
        self.register("coalesce", utility_coalesce);
        self.register("if_null", utility_if_null);
        self.register("to_string", utility_to_string);
        self.register("to_int", utility_to_int);
        self.register("to_float", utility_to_float);
        self.register("type_of", utility_type_of);
    }
}

impl Default for FunctionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ========== STRING FUNCTIONS ==========

/// Trim whitespace from both ends of a string
fn string_trim(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "trim() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::String(s) => Ok(Value::String(s.trim().to_string())),
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "string".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Convert string to uppercase
fn string_upper(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "upper() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::String(s) => Ok(Value::String(s.to_uppercase())),
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "string".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Convert string to lowercase
fn string_lower(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "lower() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::String(s) => Ok(Value::String(s.to_lowercase())),
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "string".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Get string length
fn string_length(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "length() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::String(s) => Ok(Value::Integer(s.len() as i64)),
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "string".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Extract substring
fn string_substring(args: &[Value]) -> OQLResult<Value> {
    if args.len() < 2 || args.len() > 3 {
        return Err(OQLError::Generic {
            message: "substring() expects 2 or 3 arguments: substring(string, start) or substring(string, start, length)".to_string(),
        });
    }

    let string = match &args[0] {
        Value::String(s) => s,
        Value::Null => return Ok(Value::Null),
        _ => {
            return Err(OQLError::TypeError {
                expected: "string".to_string(),
                actual: args[0].type_name().to_string(),
            })
        }
    };

    // Positions count characters, not bytes, and are clamped to the string:
    // a start before the beginning is the beginning, and a length that runs
    // past the end (or is negative) takes what is there.
    let start = match &args[1] {
        Value::Integer(i) => usize::try_from(*i).unwrap_or(0),
        _ => {
            return Err(OQLError::TypeError {
                expected: "integer".to_string(),
                actual: args[1].type_name().to_string(),
            })
        }
    };

    let length = match args.get(2) {
        None => usize::MAX,
        Some(Value::Integer(i)) => usize::try_from(*i).unwrap_or(0),
        Some(other) => {
            return Err(OQLError::TypeError {
                expected: "integer".to_string(),
                actual: other.type_name().to_string(),
            })
        }
    };

    let result: String = string.chars().skip(start).take(length).collect();

    Ok(Value::String(result))
}

/// Concatenate strings
fn string_concat(args: &[Value]) -> OQLResult<Value> {
    if args.is_empty() {
        return Ok(Value::String(String::new()));
    }

    let mut result = String::new();
    for arg in args {
        match arg {
            Value::String(s) => result.push_str(s),
            Value::Integer(i) => result.push_str(&i.to_string()),
            Value::Float(f) => result.push_str(&f.to_string()),
            Value::Boolean(b) => result.push_str(&b.to_string()),
            Value::Null => {} // Skip null values
            _ => result.push_str(&arg.to_string_repr()),
        }
    }

    Ok(Value::String(result))
}

/// Replace substring
fn string_replace(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 3 {
        return Err(OQLError::Generic {
            message: "replace() expects exactly 3 arguments: replace(string, from, to)".to_string(),
        });
    }

    let string = match &args[0] {
        Value::String(s) => s,
        Value::Null => return Ok(Value::Null),
        _ => {
            return Err(OQLError::TypeError {
                expected: "string".to_string(),
                actual: args[0].type_name().to_string(),
            })
        }
    };

    let from = match &args[1] {
        Value::String(s) => s,
        _ => {
            return Err(OQLError::TypeError {
                expected: "string".to_string(),
                actual: args[1].type_name().to_string(),
            })
        }
    };

    let to = match &args[2] {
        Value::String(s) => s,
        _ => {
            return Err(OQLError::TypeError {
                expected: "string".to_string(),
                actual: args[2].type_name().to_string(),
            })
        }
    };

    Ok(Value::String(string.replace(from, to)))
}

/// Split string into array
fn string_split(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 2 {
        return Err(OQLError::Generic {
            message: "split() expects exactly 2 arguments: split(string, delimiter)".to_string(),
        });
    }

    let string = match &args[0] {
        Value::String(s) => s,
        Value::Null => return Ok(Value::Null),
        _ => {
            return Err(OQLError::TypeError {
                expected: "string".to_string(),
                actual: args[0].type_name().to_string(),
            })
        }
    };

    let delimiter = match &args[1] {
        Value::String(s) => s,
        _ => {
            return Err(OQLError::TypeError {
                expected: "string".to_string(),
                actual: args[1].type_name().to_string(),
            })
        }
    };

    // For now, return a string representation of the array
    // In a full implementation, we'd need a List/Array value type
    let parts: Vec<&str> = string.split(delimiter).collect();
    let result = format!(
        "[{}]",
        parts
            .iter()
            .map(|s| format!("\"{}\"", s))
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(Value::String(result))
}

/// Check if string starts with prefix
fn string_starts_with(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 2 {
        return Err(OQLError::Generic {
            message: "starts_with() expects exactly 2 arguments: starts_with(string, prefix)"
                .to_string(),
        });
    }

    let string = match &args[0] {
        Value::String(s) => s,
        Value::Null => return Ok(Value::Null),
        _ => {
            return Err(OQLError::TypeError {
                expected: "string".to_string(),
                actual: args[0].type_name().to_string(),
            })
        }
    };

    let prefix = match &args[1] {
        Value::String(s) => s,
        _ => {
            return Err(OQLError::TypeError {
                expected: "string".to_string(),
                actual: args[1].type_name().to_string(),
            })
        }
    };

    Ok(Value::Boolean(string.starts_with(prefix)))
}

/// Check if string ends with suffix
fn string_ends_with(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 2 {
        return Err(OQLError::Generic {
            message: "ends_with() expects exactly 2 arguments: ends_with(string, suffix)"
                .to_string(),
        });
    }

    let string = match &args[0] {
        Value::String(s) => s,
        Value::Null => return Ok(Value::Null),
        _ => {
            return Err(OQLError::TypeError {
                expected: "string".to_string(),
                actual: args[0].type_name().to_string(),
            })
        }
    };

    let suffix = match &args[1] {
        Value::String(s) => s,
        _ => {
            return Err(OQLError::TypeError {
                expected: "string".to_string(),
                actual: args[1].type_name().to_string(),
            })
        }
    };

    Ok(Value::Boolean(string.ends_with(suffix)))
}

/// Check if string contains substring
fn string_contains(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 2 {
        return Err(OQLError::Generic {
            message: "contains() expects exactly 2 arguments: contains(string, substring)"
                .to_string(),
        });
    }

    let string = match &args[0] {
        Value::String(s) => s,
        Value::Null => return Ok(Value::Null),
        _ => {
            return Err(OQLError::TypeError {
                expected: "string".to_string(),
                actual: args[0].type_name().to_string(),
            })
        }
    };

    let substring = match &args[1] {
        Value::String(s) => s,
        _ => {
            return Err(OQLError::TypeError {
                expected: "string".to_string(),
                actual: args[1].type_name().to_string(),
            })
        }
    };

    Ok(Value::Boolean(string.contains(substring)))
}

/// Longest string a padding function will produce. A query must not be able
/// to request an arbitrarily large allocation with one integer.
const MAX_PADDED_LENGTH: usize = 1 << 20;

fn padded_length(requested: i64) -> OQLResult<usize> {
    let length = usize::try_from(requested).unwrap_or(0);
    if length > MAX_PADDED_LENGTH {
        return Err(OQLError::FunctionError {
            message: format!(
                "padding to {requested} characters exceeds the limit of {MAX_PADDED_LENGTH}"
            ),
        });
    }
    Ok(length)
}

/// Pad string on the left
fn string_pad_left(args: &[Value]) -> OQLResult<Value> {
    if args.len() < 2 || args.len() > 3 {
        return Err(OQLError::Generic {
            message: "pad_left() expects 2 or 3 arguments: pad_left(string, length) or pad_left(string, length, pad_char)".to_string(),
        });
    }

    let string = match &args[0] {
        Value::String(s) => s,
        Value::Null => return Ok(Value::Null),
        _ => {
            return Err(OQLError::TypeError {
                expected: "string".to_string(),
                actual: args[0].type_name().to_string(),
            })
        }
    };

    let length = match &args[1] {
        Value::Integer(i) => padded_length(*i)?,
        _ => {
            return Err(OQLError::TypeError {
                expected: "integer".to_string(),
                actual: args[1].type_name().to_string(),
            })
        }
    };

    let pad_char = if args.len() == 3 {
        match &args[2] {
            Value::String(s) => s.chars().next().unwrap_or(' '),
            _ => {
                return Err(OQLError::TypeError {
                    expected: "string".to_string(),
                    actual: args[2].type_name().to_string(),
                })
            }
        }
    } else {
        ' '
    };

    let current = string.chars().count();
    if current >= length {
        Ok(Value::String(string.clone()))
    } else {
        let padding = pad_char.to_string().repeat(length - current);
        Ok(Value::String(format!("{}{}", padding, string)))
    }
}

/// Pad string on the right
fn string_pad_right(args: &[Value]) -> OQLResult<Value> {
    if args.len() < 2 || args.len() > 3 {
        return Err(OQLError::Generic {
            message: "pad_right() expects 2 or 3 arguments: pad_right(string, length) or pad_right(string, length, pad_char)".to_string(),
        });
    }

    let string = match &args[0] {
        Value::String(s) => s,
        Value::Null => return Ok(Value::Null),
        _ => {
            return Err(OQLError::TypeError {
                expected: "string".to_string(),
                actual: args[0].type_name().to_string(),
            })
        }
    };

    let length = match &args[1] {
        Value::Integer(i) => padded_length(*i)?,
        _ => {
            return Err(OQLError::TypeError {
                expected: "integer".to_string(),
                actual: args[1].type_name().to_string(),
            })
        }
    };

    let pad_char = if args.len() == 3 {
        match &args[2] {
            Value::String(s) => s.chars().next().unwrap_or(' '),
            _ => {
                return Err(OQLError::TypeError {
                    expected: "string".to_string(),
                    actual: args[2].type_name().to_string(),
                })
            }
        }
    } else {
        ' '
    };

    let current = string.chars().count();
    if current >= length {
        Ok(Value::String(string.clone()))
    } else {
        let padding = pad_char.to_string().repeat(length - current);
        Ok(Value::String(format!("{}{}", string, padding)))
    }
}

// ========== MATH FUNCTIONS ==========

/// Absolute value
fn math_abs(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "abs() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::Integer(i) => Ok(Value::Integer(i.abs())),
        Value::Float(f) => Ok(Value::Float(f.abs())),
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "numeric type".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Round to nearest integer
fn math_round(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "round() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::Integer(i) => Ok(Value::Integer(*i)),
        Value::Float(f) => Ok(Value::Integer(f.round() as i64)),
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "numeric type".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Floor (round down)
fn math_floor(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "floor() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::Integer(i) => Ok(Value::Integer(*i)),
        Value::Float(f) => Ok(Value::Integer(f.floor() as i64)),
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "numeric type".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Ceiling (round up)
fn math_ceil(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "ceil() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::Integer(i) => Ok(Value::Integer(*i)),
        Value::Float(f) => Ok(Value::Integer(f.ceil() as i64)),
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "numeric type".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Square root
fn math_sqrt(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "sqrt() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::Integer(i) => {
            if *i < 0 {
                Err(OQLError::Generic {
                    message: "sqrt() of negative number".to_string(),
                })
            } else {
                Ok(Value::Float((*i as f64).sqrt()))
            }
        }
        Value::Float(f) => {
            if *f < 0.0 {
                Err(OQLError::Generic {
                    message: "sqrt() of negative number".to_string(),
                })
            } else {
                Ok(Value::Float(f.sqrt()))
            }
        }
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "numeric type".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Power function
fn math_pow(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 2 {
        return Err(OQLError::Generic {
            message: "pow() expects exactly 2 arguments: pow(base, exponent)".to_string(),
        });
    }

    let base = match &args[0] {
        Value::Integer(i) => *i as f64,
        Value::Float(f) => *f,
        Value::Null => return Ok(Value::Null),
        _ => {
            return Err(OQLError::TypeError {
                expected: "numeric type".to_string(),
                actual: args[0].type_name().to_string(),
            })
        }
    };

    let exponent = match &args[1] {
        Value::Integer(i) => *i as f64,
        Value::Float(f) => *f,
        Value::Null => return Ok(Value::Null),
        _ => {
            return Err(OQLError::TypeError {
                expected: "numeric type".to_string(),
                actual: args[1].type_name().to_string(),
            })
        }
    };

    Ok(Value::Float(base.powf(exponent)))
}

/// Minimum of two values
fn math_min(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 2 {
        return Err(OQLError::Generic {
            message: "min() expects exactly 2 arguments".to_string(),
        });
    }

    match (&args[0], &args[1]) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(*a.min(b))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a.min(*b))),
        (Value::Integer(a), Value::Float(b)) => Ok(Value::Float((*a as f64).min(*b))),
        (Value::Float(a), Value::Integer(b)) => Ok(Value::Float(a.min(*b as f64))),
        _ => Err(OQLError::TypeError {
            expected: "numeric types".to_string(),
            actual: format!("{} and {}", args[0].type_name(), args[1].type_name()),
        }),
    }
}

/// Maximum of two values
fn math_max(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 2 {
        return Err(OQLError::Generic {
            message: "max() expects exactly 2 arguments".to_string(),
        });
    }

    match (&args[0], &args[1]) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        (Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(*a.max(b))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a.max(*b))),
        (Value::Integer(a), Value::Float(b)) => Ok(Value::Float((*a as f64).max(*b))),
        (Value::Float(a), Value::Integer(b)) => Ok(Value::Float(a.max(*b as f64))),
        _ => Err(OQLError::TypeError {
            expected: "numeric types".to_string(),
            actual: format!("{} and {}", args[0].type_name(), args[1].type_name()),
        }),
    }
}

/// Sine function
fn math_sin(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "sin() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::Integer(i) => Ok(Value::Float((*i as f64).sin())),
        Value::Float(f) => Ok(Value::Float(f.sin())),
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "numeric type".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Cosine function
fn math_cos(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "cos() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::Integer(i) => Ok(Value::Float((*i as f64).cos())),
        Value::Float(f) => Ok(Value::Float(f.cos())),
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "numeric type".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Tangent function
fn math_tan(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "tan() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::Integer(i) => Ok(Value::Float((*i as f64).tan())),
        Value::Float(f) => Ok(Value::Float(f.tan())),
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "numeric type".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Logarithm base 10
fn math_log(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "log() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::Integer(i) => {
            if *i <= 0 {
                Err(OQLError::Generic {
                    message: "log() of non-positive number".to_string(),
                })
            } else {
                Ok(Value::Float((*i as f64).log10()))
            }
        }
        Value::Float(f) => {
            if *f <= 0.0 {
                Err(OQLError::Generic {
                    message: "log() of non-positive number".to_string(),
                })
            } else {
                Ok(Value::Float(f.log10()))
            }
        }
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "numeric type".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Natural logarithm
fn math_ln(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "ln() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::Integer(i) => {
            if *i <= 0 {
                Err(OQLError::Generic {
                    message: "ln() of non-positive number".to_string(),
                })
            } else {
                Ok(Value::Float((*i as f64).ln()))
            }
        }
        Value::Float(f) => {
            if *f <= 0.0 {
                Err(OQLError::Generic {
                    message: "ln() of non-positive number".to_string(),
                })
            } else {
                Ok(Value::Float(f.ln()))
            }
        }
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "numeric type".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Exponential function (e^x)
fn math_exp(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "exp() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::Integer(i) => Ok(Value::Float((*i as f64).exp())),
        Value::Float(f) => Ok(Value::Float(f.exp())),
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "numeric type".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Random number between 0 and 1
fn math_random(args: &[Value]) -> OQLResult<Value> {
    if !args.is_empty() {
        return Err(OQLError::Generic {
            message: "random() expects no arguments".to_string(),
        });
    }

    use rand::Rng;
    let mut rng = rand::thread_rng();
    Ok(Value::Float(rng.gen::<f64>()))
}

// ========== DATE/TIME FUNCTIONS ==========

/// Get current date and time
fn datetime_now(args: &[Value]) -> OQLResult<Value> {
    if !args.is_empty() {
        return Err(OQLError::Generic {
            message: "now() expects no arguments".to_string(),
        });
    }

    Ok(Value::DateTime(Utc::now()))
}

/// Get current date (midnight)
fn datetime_today(args: &[Value]) -> OQLResult<Value> {
    if !args.is_empty() {
        return Err(OQLError::Generic {
            message: "today() expects no arguments".to_string(),
        });
    }

    let now = Utc::now();
    let today = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
    Ok(Value::DateTime(Utc.from_utc_datetime(&today)))
}

/// Get date N days ago
fn datetime_days_ago(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "days_ago() expects exactly 1 argument".to_string(),
        });
    }

    let days = match &args[0] {
        Value::Integer(i) => *i,
        _ => {
            return Err(OQLError::TypeError {
                expected: "integer".to_string(),
                actual: args[0].type_name().to_string(),
            })
        }
    };

    let now = Utc::now();
    let past = now - Duration::days(days);
    Ok(Value::DateTime(past))
}

/// Get date N hours ago
fn datetime_hours_ago(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "hours_ago() expects exactly 1 argument".to_string(),
        });
    }

    let hours = match &args[0] {
        Value::Integer(i) => *i,
        _ => {
            return Err(OQLError::TypeError {
                expected: "integer".to_string(),
                actual: args[0].type_name().to_string(),
            })
        }
    };

    let now = Utc::now();
    let past = now - Duration::hours(hours);
    Ok(Value::DateTime(past))
}

/// Get date N minutes ago
fn datetime_minutes_ago(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "minutes_ago() expects exactly 1 argument".to_string(),
        });
    }

    let minutes = match &args[0] {
        Value::Integer(i) => *i,
        _ => {
            return Err(OQLError::TypeError {
                expected: "integer".to_string(),
                actual: args[0].type_name().to_string(),
            })
        }
    };

    let now = Utc::now();
    let past = now - Duration::minutes(minutes);
    Ok(Value::DateTime(past))
}

/// Format date as string
fn datetime_format(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 2 {
        return Err(OQLError::Generic {
            message: "format_date() expects exactly 2 arguments: format_date(datetime, format)"
                .to_string(),
        });
    }

    let datetime = match &args[0] {
        Value::DateTime(dt) => dt,
        Value::Null => return Ok(Value::Null),
        _ => {
            return Err(OQLError::TypeError {
                expected: "datetime".to_string(),
                actual: args[0].type_name().to_string(),
            })
        }
    };

    let format = match &args[1] {
        Value::String(s) => s,
        _ => {
            return Err(OQLError::TypeError {
                expected: "string".to_string(),
                actual: args[1].type_name().to_string(),
            })
        }
    };

    Ok(Value::String(datetime.format(format).to_string()))
}

/// Parse date from string
fn datetime_parse(args: &[Value]) -> OQLResult<Value> {
    if args.len() < 1 || args.len() > 2 {
        return Err(OQLError::Generic {
            message: "parse_date() expects 1 or 2 arguments: parse_date(string) or parse_date(string, format)".to_string(),
        });
    }

    let date_str = match &args[0] {
        Value::String(s) => s,
        Value::Null => return Ok(Value::Null),
        _ => {
            return Err(OQLError::TypeError {
                expected: "string".to_string(),
                actual: args[0].type_name().to_string(),
            })
        }
    };

    // Try common formats if no format specified
    if args.len() == 1 {
        // Try ISO 8601 format first
        if let Ok(dt) = DateTime::parse_from_rfc3339(date_str) {
            return Ok(Value::DateTime(dt.with_timezone(&Utc)));
        }

        // Try other common formats
        let formats = ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d", "%m/%d/%Y", "%d/%m/%Y"];

        for format in &formats {
            if let Ok(naive_dt) = NaiveDateTime::parse_from_str(date_str, format) {
                return Ok(Value::DateTime(Utc.from_utc_datetime(&naive_dt)));
            }
        }

        return Err(OQLError::Generic {
            message: format!("Unable to parse date: {}", date_str),
        });
    } else {
        let format = match &args[1] {
            Value::String(s) => s,
            _ => {
                return Err(OQLError::TypeError {
                    expected: "string".to_string(),
                    actual: args[1].type_name().to_string(),
                })
            }
        };

        match NaiveDateTime::parse_from_str(date_str, format) {
            Ok(naive_dt) => Ok(Value::DateTime(Utc.from_utc_datetime(&naive_dt))),
            Err(_) => Err(OQLError::Generic {
                message: format!(
                    "Unable to parse date '{}' with format '{}'",
                    date_str, format
                ),
            }),
        }
    }
}

/// Extract year from date
fn datetime_year(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "year() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::DateTime(dt) => Ok(Value::Integer(dt.year() as i64)),
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "datetime".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Extract month from date
fn datetime_month(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "month() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::DateTime(dt) => Ok(Value::Integer(dt.month() as i64)),
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "datetime".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Extract day from date
fn datetime_day(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "day() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::DateTime(dt) => Ok(Value::Integer(dt.day() as i64)),
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "datetime".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Extract hour from date
fn datetime_hour(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "hour() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::DateTime(dt) => Ok(Value::Integer(dt.hour() as i64)),
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "datetime".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Extract minute from date
fn datetime_minute(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "minute() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::DateTime(dt) => Ok(Value::Integer(dt.minute() as i64)),
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "datetime".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Extract second from date
fn datetime_second(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "second() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::DateTime(dt) => Ok(Value::Integer(dt.second() as i64)),
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "datetime".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Get weekday (0 = Sunday, 6 = Saturday)
fn datetime_weekday(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "weekday() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::DateTime(dt) => Ok(Value::Integer(dt.weekday().num_days_from_sunday() as i64)),
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "datetime".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Add days to date
fn datetime_add_days(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 2 {
        return Err(OQLError::Generic {
            message: "add_days() expects exactly 2 arguments: add_days(datetime, days)".to_string(),
        });
    }

    let datetime = match &args[0] {
        Value::DateTime(dt) => dt,
        Value::Null => return Ok(Value::Null),
        _ => {
            return Err(OQLError::TypeError {
                expected: "datetime".to_string(),
                actual: args[0].type_name().to_string(),
            })
        }
    };

    let days = match &args[1] {
        Value::Integer(i) => *i,
        _ => {
            return Err(OQLError::TypeError {
                expected: "integer".to_string(),
                actual: args[1].type_name().to_string(),
            })
        }
    };

    let result = *datetime + Duration::days(days);
    Ok(Value::DateTime(result))
}

/// Add hours to date
fn datetime_add_hours(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 2 {
        return Err(OQLError::Generic {
            message: "add_hours() expects exactly 2 arguments: add_hours(datetime, hours)"
                .to_string(),
        });
    }

    let datetime = match &args[0] {
        Value::DateTime(dt) => dt,
        Value::Null => return Ok(Value::Null),
        _ => {
            return Err(OQLError::TypeError {
                expected: "datetime".to_string(),
                actual: args[0].type_name().to_string(),
            })
        }
    };

    let hours = match &args[1] {
        Value::Integer(i) => *i,
        _ => {
            return Err(OQLError::TypeError {
                expected: "integer".to_string(),
                actual: args[1].type_name().to_string(),
            })
        }
    };

    let result = *datetime + Duration::hours(hours);
    Ok(Value::DateTime(result))
}

/// Calculate difference in days between two dates
fn datetime_diff_days(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 2 {
        return Err(OQLError::Generic {
            message: "diff_days() expects exactly 2 arguments: diff_days(datetime1, datetime2)"
                .to_string(),
        });
    }

    let dt1 = match &args[0] {
        Value::DateTime(dt) => dt,
        Value::Null => return Ok(Value::Null),
        _ => {
            return Err(OQLError::TypeError {
                expected: "datetime".to_string(),
                actual: args[0].type_name().to_string(),
            })
        }
    };

    let dt2 = match &args[1] {
        Value::DateTime(dt) => dt,
        Value::Null => return Ok(Value::Null),
        _ => {
            return Err(OQLError::TypeError {
                expected: "datetime".to_string(),
                actual: args[1].type_name().to_string(),
            })
        }
    };

    let diff = *dt2 - *dt1;
    Ok(Value::Integer(diff.num_days()))
}

// ========== UTILITY FUNCTIONS ==========

/// Return first non-null value
fn utility_coalesce(args: &[Value]) -> OQLResult<Value> {
    if args.is_empty() {
        return Err(OQLError::Generic {
            message: "coalesce() expects at least 1 argument".to_string(),
        });
    }

    for arg in args {
        if !arg.is_null() {
            return Ok(arg.clone());
        }
    }

    Ok(Value::Null)
}

/// Return default value if first argument is null
fn utility_if_null(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 2 {
        return Err(OQLError::Generic {
            message: "if_null() expects exactly 2 arguments: if_null(value, default)".to_string(),
        });
    }

    if args[0].is_null() {
        Ok(args[1].clone())
    } else {
        Ok(args[0].clone())
    }
}

/// Convert value to string
fn utility_to_string(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "to_string() expects exactly 1 argument".to_string(),
        });
    }

    Ok(Value::String(args[0].to_string_repr()))
}

/// Convert value to integer
fn utility_to_int(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "to_int() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::Integer(i) => Ok(Value::Integer(*i)),
        Value::Float(f) => Ok(Value::Integer(*f as i64)),
        Value::String(s) => match s.parse::<i64>() {
            Ok(i) => Ok(Value::Integer(i)),
            Err(_) => Err(OQLError::Generic {
                message: format!("Cannot convert '{}' to integer", s),
            }),
        },
        Value::Boolean(b) => Ok(Value::Integer(if *b { 1 } else { 0 })),
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "convertible type".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Convert value to float
fn utility_to_float(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "to_float() expects exactly 1 argument".to_string(),
        });
    }

    match &args[0] {
        Value::Integer(i) => Ok(Value::Float(*i as f64)),
        Value::Float(f) => Ok(Value::Float(*f)),
        Value::String(s) => match s.parse::<f64>() {
            Ok(f) => Ok(Value::Float(f)),
            Err(_) => Err(OQLError::Generic {
                message: format!("Cannot convert '{}' to float", s),
            }),
        },
        Value::Boolean(b) => Ok(Value::Float(if *b { 1.0 } else { 0.0 })),
        Value::Null => Ok(Value::Null),
        _ => Err(OQLError::TypeError {
            expected: "convertible type".to_string(),
            actual: args[0].type_name().to_string(),
        }),
    }
}

/// Get type name of value
fn utility_type_of(args: &[Value]) -> OQLResult<Value> {
    if args.len() != 1 {
        return Err(OQLError::Generic {
            message: "type_of() expects exactly 1 argument".to_string(),
        });
    }

    Ok(Value::String(args[0].type_name().to_string()))
}
