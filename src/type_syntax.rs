use crate::error::{OQLError, OQLResult};
use crate::schema::{FieldType, ReferenceType};

pub fn parse_declared_field_type<F>(type_str: &str, schema_exists: F) -> OQLResult<FieldType>
where
    F: Fn(&str) -> bool + Copy,
{
    let raw = type_str.trim();
    if raw.is_empty() {
        return Err(OQLError::ParseError {
            message: "field type cannot be empty".to_string(),
        });
    }

    if raw.ends_with('?') {
        let inner = parse_declared_field_type(&raw[..raw.len() - 1], schema_exists)?;
        return Ok(FieldType::Optional(Box::new(inner)));
    }

    if let Some(inner) = raw.strip_prefix("List(").and_then(|s| s.strip_suffix(')')) {
        let inner_type = parse_declared_field_type(inner, schema_exists)?;
        return Ok(FieldType::List(Box::new(inner_type)));
    }
    if let Some(inner) = raw.strip_prefix("Array(").and_then(|s| s.strip_suffix(')')) {
        let inner_type = parse_declared_field_type(inner, schema_exists)?;
        return Ok(FieldType::List(Box::new(inner_type)));
    }
    if let Some(inner) = raw
        .strip_prefix("Optional(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let inner_type = parse_declared_field_type(inner, schema_exists)?;
        return Ok(FieldType::Optional(Box::new(inner_type)));
    }
    if let Some(dim_raw) = raw
        .strip_prefix("Vector(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let dim = dim_raw
            .trim()
            .parse::<u32>()
            .map_err(|_| OQLError::ParseError {
                message: format!("Invalid Vector dimension '{}'", dim_raw.trim()),
            })?;
        if dim == 0 {
            return Err(OQLError::ParseError {
                message: "Vector dimension must be > 0".to_string(),
            });
        }
        return Ok(FieldType::Vector { dim });
    }
    if let Some(target) = raw.strip_prefix("Ref(").and_then(|s| s.strip_suffix(')')) {
        return Ok(FieldType::Reference(ReferenceType::new(
            target.trim().to_string(),
        )));
    }
    if let Some(target) = raw
        .strip_prefix("Reference(")
        .and_then(|s| s.strip_suffix(')'))
    {
        return Ok(FieldType::Reference(ReferenceType::new(
            target.trim().to_string(),
        )));
    }

    let lower = raw.to_ascii_lowercase();
    match lower.as_str() {
        "string" => Ok(FieldType::String),
        "integer" | "int" | "i64" => Ok(FieldType::Integer),
        "float" | "double" | "f64" => Ok(FieldType::Float),
        "boolean" | "bool" => Ok(FieldType::Boolean),
        "datetime" | "timestamp" => Ok(FieldType::DateTime),
        "object" | "map" | "json" => Ok(FieldType::Object),
        _ if schema_exists(raw) => Ok(FieldType::Reference(ReferenceType::new(raw.to_string()))),
        _ => Err(OQLError::ParseError {
            message: format!("Unknown field type '{}'", raw),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vector_type_with_dimension() {
        let parsed = parse_declared_field_type("Vector(384)", |_| false).unwrap();
        assert_eq!(parsed, FieldType::Vector { dim: 384 });
    }

    #[test]
    fn rejects_zero_dimension_vector() {
        let err = parse_declared_field_type("Vector(0)", |_| false).unwrap_err();
        assert!(err.to_string().contains("Vector dimension must be > 0"));
    }
}
