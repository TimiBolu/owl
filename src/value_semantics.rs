//! The single definition of how OQL values compare.
//!
//! Every execution path — the general executor, the physical read pipeline,
//! and nested collection evaluation — must agree on what `==`, `<`, `in`,
//! and `sort` mean, or the same query returns different rows depending on
//! which path happened to run it. They all call into this module.
//!
//! The rules follow `docs/OWL_NULL_AND_EXISTENCE_SEMANTICS.md`:
//!
//! - A comparison never fails. A predicate that cannot be decided for a row
//!   simply does not match that row.
//! - `null == null` is true and `null != x` is true for any non-null `x`.
//!   Every ordered comparison (`<`, `<=`, `>`, `>=`) involving `null` is
//!   false.
//! - Integers and floats compare by numeric value: `10 == 10.0`.
//! - A `DateTime` compares with a string that parses as an RFC 3339
//!   timestamp, by instant rather than by text.
//! - Values of otherwise different types are unequal and unordered.
//! - `NaN` equals nothing and is unordered.
//! - Sorting uses a separate total order so it is deterministic for any
//!   input: nulls first, then values grouped by type.

use crate::document::Value;
use crate::parser::ComparisonOperator;
use chrono::{DateTime, Utc};
use std::cmp::Ordering;

/// Read a timestamp written as text: RFC 3339 (`2026-01-05T10:00:00Z`), or a
/// calendar date (`2026-01-05`), which means midnight UTC on that day.
pub fn parse_timestamp(text: &str) -> Option<DateTime<Utc>> {
    if let Ok(instant) = DateTime::parse_from_rfc3339(text) {
        return Some(instant.with_timezone(&Utc));
    }
    chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|midnight| midnight.and_utc())
}

fn parse_instant(text: &str) -> Option<DateTime<Utc>> {
    parse_timestamp(text)
}

/// Compare an integer with a float without rounding the integer first, so
/// integers beyond 2^53 still order correctly against nearby floats.
fn compare_integer_float(integer: i64, float: f64) -> Option<Ordering> {
    if float.is_nan() {
        return None;
    }
    // Every i64 lies strictly inside this range, so anything outside it
    // (including the infinities) is decided by sign alone.
    if float >= 9_223_372_036_854_775_808.0 {
        return Some(Ordering::Less);
    }
    if float < -9_223_372_036_854_775_808.0 {
        return Some(Ordering::Greater);
    }
    let truncated = float.trunc();
    match integer.cmp(&(truncated as i64)) {
        Ordering::Equal => {
            let fraction = float - truncated;
            if fraction > 0.0 {
                Some(Ordering::Less)
            } else if fraction < 0.0 {
                Some(Ordering::Greater)
            } else {
                Some(Ordering::Equal)
            }
        }
        decided => Some(decided),
    }
}

/// The natural order of two values, or `None` when they cannot be ordered:
/// either side is null, the types are unrelated, or a float is `NaN`.
pub fn compare(left: &Value, right: &Value) -> Option<Ordering> {
    match (left, right) {
        (Value::Integer(a), Value::Integer(b)) => Some(a.cmp(b)),
        (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
        (Value::Integer(a), Value::Float(b)) => compare_integer_float(*a, *b),
        (Value::Float(a), Value::Integer(b)) => {
            compare_integer_float(*b, *a).map(Ordering::reverse)
        }
        (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
        (Value::Boolean(a), Value::Boolean(b)) => Some(a.cmp(b)),
        (Value::DateTime(a), Value::DateTime(b)) => Some(a.cmp(b)),
        (Value::DateTime(a), Value::String(b)) => parse_instant(b).map(|b| a.cmp(&b)),
        (Value::String(a), Value::DateTime(b)) => parse_instant(a).map(|a| a.cmp(b)),
        _ => None,
    }
}

/// OQL equality.
pub fn equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Reference(a), Value::Reference(b)) => a == b,
        (Value::GlobalReference(a), Value::GlobalReference(b)) => a == b,
        (Value::List(a), Value::List(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equal(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(name, value)| b.get(name).is_some_and(|other| equal(value, other)))
        }
        _ => compare(left, right) == Some(Ordering::Equal),
    }
}

/// Decide `left <operator> right`. Never fails: an undecidable comparison is
/// a non-match.
pub fn matches(left: &Value, operator: &ComparisonOperator, right: &Value) -> bool {
    match operator {
        ComparisonOperator::Equal => equal(left, right),
        ComparisonOperator::NotEqual => !equal(left, right),
        ComparisonOperator::LessThan => compare(left, right) == Some(Ordering::Less),
        ComparisonOperator::GreaterThan => compare(left, right) == Some(Ordering::Greater),
        ComparisonOperator::LessEqual => {
            matches!(compare(left, right), Some(Ordering::Less | Ordering::Equal))
        }
        ComparisonOperator::GreaterEqual => {
            matches!(
                compare(left, right),
                Some(Ordering::Greater | Ordering::Equal)
            )
        }
        ComparisonOperator::Contains => match (left, right) {
            (Value::String(text), Value::String(part)) => text.contains(part.as_str()),
            // A list contains a value when one of its elements equals it, so
            // `.tags contains "beta"` reads the same as `"beta" in .tags`.
            (Value::List(items), element) => items.iter().any(|item| equal(item, element)),
            _ => false,
        },
        ComparisonOperator::StartsWith => match (left, right) {
            (Value::String(text), Value::String(prefix)) => text.starts_with(prefix.as_str()),
            _ => false,
        },
        ComparisonOperator::EndsWith => match (left, right) {
            (Value::String(text), Value::String(suffix)) => text.ends_with(suffix.as_str()),
            _ => false,
        },
        ComparisonOperator::In => match right {
            Value::List(items) => items.iter().any(|item| equal(left, item)),
            _ => false,
        },
    }
}

/// Position of a type in the sort order. Integers and floats share a rank
/// because they sort together by numeric value.
fn sort_rank(value: &Value) -> u8 {
    match value {
        Value::Null => 0,
        Value::Boolean(_) => 1,
        Value::Integer(_) | Value::Float(_) => 2,
        Value::String(_) => 3,
        Value::DateTime(_) => 4,
        Value::Reference(_) | Value::GlobalReference(_) => 5,
        Value::List(_) => 6,
        Value::Object(_) => 7,
    }
}

/// A total order for `sort`: nulls first, then by type, then by value.
/// Unlike [`compare`], this always returns an answer, so sorting any mix of
/// values is deterministic and can never violate the sort contract.
pub fn sort_order(left: &Value, right: &Value) -> Ordering {
    let ranks = sort_rank(left).cmp(&sort_rank(right));
    if ranks != Ordering::Equal {
        return ranks;
    }
    match (left, right) {
        (Value::Float(a), Value::Float(b)) => a.total_cmp(b),
        // NaN sorts after every number, matching `f64::total_cmp` for the
        // positive NaN that arithmetic produces.
        (Value::Integer(_), Value::Float(b)) if b.is_nan() => Ordering::Less,
        (Value::Float(a), Value::Integer(_)) if a.is_nan() => Ordering::Greater,
        (Value::List(a), Value::List(b)) => a
            .iter()
            .zip(b)
            .map(|(a, b)| sort_order(a, b))
            .find(|ordering| *ordering != Ordering::Equal)
            .unwrap_or_else(|| a.len().cmp(&b.len())),
        _ => compare(left, right).unwrap_or(Ordering::Equal),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ComparisonOperator::*;

    fn int(value: i64) -> Value {
        Value::Integer(value)
    }

    fn float(value: f64) -> Value {
        Value::Float(value)
    }

    fn text(value: &str) -> Value {
        Value::String(value.to_string())
    }

    #[test]
    fn integers_and_floats_compare_by_numeric_value() {
        assert!(matches(&float(10.0), &Equal, &int(10)));
        assert!(matches(&int(10), &Equal, &float(10.0)));
        assert!(matches(&float(10.0), &GreaterEqual, &int(10)));
        assert!(matches(&float(10.0), &LessEqual, &int(10)));
        assert!(matches(&int(10), &LessThan, &float(10.5)));
        assert!(!matches(&int(10), &Equal, &float(10.5)));
        assert!(matches(&int(10), &NotEqual, &float(10.5)));
    }

    #[test]
    fn large_integers_are_not_rounded_before_comparing() {
        let big = (1i64 << 53) + 1;
        assert!(matches(
            &int(big),
            &GreaterThan,
            &float((1i64 << 53) as f64)
        ));
        assert!(!matches(&int(big), &Equal, &float((1i64 << 53) as f64)));
        assert!(matches(&int(i64::MAX), &LessThan, &float(f64::INFINITY)));
        assert!(matches(
            &int(i64::MIN),
            &GreaterThan,
            &float(f64::NEG_INFINITY)
        ));
    }

    #[test]
    fn null_never_orders_and_equals_only_null() {
        for operator in [LessThan, LessEqual, GreaterThan, GreaterEqual] {
            assert!(!matches(&Value::Null, &operator, &int(1)));
            assert!(!matches(&int(1), &operator, &Value::Null));
            assert!(!matches(&Value::Null, &operator, &Value::Null));
        }
        assert!(matches(&Value::Null, &Equal, &Value::Null));
        assert!(!matches(&Value::Null, &Equal, &int(0)));
        assert!(matches(&int(0), &NotEqual, &Value::Null));
    }

    #[test]
    fn unrelated_types_do_not_match_instead_of_failing() {
        assert!(!matches(&text("10"), &Equal, &int(10)));
        assert!(!matches(&text("10"), &LessThan, &int(11)));
        assert!(!matches(&text("10"), &GreaterThan, &int(9)));
        assert!(matches(&text("10"), &NotEqual, &int(10)));
    }

    #[test]
    fn nan_is_unordered_and_unequal() {
        let nan = float(f64::NAN);
        assert!(!matches(&nan, &Equal, &nan));
        assert!(!matches(&nan, &LessThan, &float(1.0)));
        assert!(!matches(&nan, &GreaterEqual, &float(1.0)));
    }

    #[test]
    fn datetimes_compare_with_timestamp_strings_by_instant() {
        let instant = Value::DateTime(parse_instant("2026-01-01T12:00:00Z").unwrap());
        assert!(matches(&instant, &Equal, &text("2026-01-01T12:00:00Z")));
        assert!(matches(
            &instant,
            &Equal,
            &text("2026-01-01T13:00:00+01:00")
        ));
        assert!(matches(
            &instant,
            &GreaterThan,
            &text("2026-01-01T11:59:59Z")
        ));
        assert!(matches(&text("2025-12-31T00:00:00Z"), &LessThan, &instant));
        assert!(!matches(&instant, &Equal, &text("not a timestamp")));
        assert!(!matches(&instant, &LessThan, &text("not a timestamp")));
    }

    #[test]
    fn membership_uses_the_same_equality() {
        let list = Value::List(vec![int(1), float(2.0), text("three")]);
        assert!(matches(&float(1.0), &In, &list));
        assert!(matches(&int(2), &In, &list));
        assert!(matches(&text("three"), &In, &list));
        assert!(!matches(&int(3), &In, &list));
        assert!(!matches(&int(1), &In, &int(1)));
        assert!(matches(&list, &Contains, &int(1)));
        assert!(matches(&list, &Contains, &text("three")));
        assert!(!matches(&list, &Contains, &text("four")));
        assert!(matches(&text("three"), &Contains, &text("hre")));
    }

    #[test]
    fn sort_order_is_total_and_puts_nulls_first() {
        let mut values = vec![
            text("b"),
            float(f64::NAN),
            int(3),
            Value::Null,
            float(2.5),
            Value::Boolean(true),
            text("a"),
            int(-1),
        ];
        values.sort_by(sort_order);
        assert_eq!(values[0], Value::Null);
        assert_eq!(values[1], Value::Boolean(true));
        assert_eq!(values[2], int(-1));
        assert_eq!(values[3], float(2.5));
        assert_eq!(values[4], int(3));
        assert!(matches!(values[5], Value::Float(nan) if nan.is_nan()));
        assert_eq!(values[6], text("a"));
        assert_eq!(values[7], text("b"));
    }
}
