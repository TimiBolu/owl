//! Vectors: what a `Vector(n)` field holds, and what can be done with one.
//!
//! A vector is a fixed number of 32-bit numbers, held and stored packed.
//! In a query it is written as a list of numbers, and wherever a vector
//! is wanted such a list is taken for one, so a vector stored as a list
//! before vectors were packed is still a vector.
//!
//! | Form | Gives |
//! |---|---|
//! | `a + b`, `a - b` | the two added or subtracted, number by number |
//! | `a * 2`, `2 * a`, `a / 2` | every number scaled |
//! | `dot(a, b)` | the sum of the products |
//! | `norm(a)` | the length |
//! | `normalize(a)` | the same direction, at length one |
//! | `similarity(a, b)` | the cosine of the angle between: 1 the same way, 0 unrelated, -1 opposite |
//! | `distance(a, b)` | the straight-line distance: 0 the same point |
//! | `centroid(vectors)` | the mean of several, number by number |
//! | `vector(list)` | a list of numbers as a vector |
//!
//! Two vectors must have the same number of numbers. A null in place of a
//! vector gives null, so a row with no embedding has no similarity and
//! is never an error.

use crate::document::Value;
use crate::error::{OQLError, OQLResult};
use crate::parser::BinaryOperator;
use std::borrow::Cow;

/// The sum of the products of two equal-length runs of numbers.
///
/// Eight running sums, so that the compiler can keep them in one wide
/// register: a single sum must be added to in order, which it cannot
/// reorder.
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut sums = [0f32; 8];
    let (a_chunks, b_chunks) = (a.chunks_exact(8), b.chunks_exact(8));
    let (a_rest, b_rest) = (a_chunks.remainder(), b_chunks.remainder());
    for (a, b) in a_chunks.zip(b_chunks) {
        for lane in 0..8 {
            sums[lane] += a[lane] * b[lane];
        }
    }
    let rest: f32 = a_rest.iter().zip(b_rest).map(|(a, b)| a * b).sum();
    sums.iter().sum::<f32>() + rest
}

/// The square of the straight-line distance between two points.
pub fn squared_distance(a: &[f32], b: &[f32]) -> f32 {
    let mut sums = [0f32; 8];
    let (a_chunks, b_chunks) = (a.chunks_exact(8), b.chunks_exact(8));
    let (a_rest, b_rest) = (a_chunks.remainder(), b_chunks.remainder());
    for (a, b) in a_chunks.zip(b_chunks) {
        for lane in 0..8 {
            let apart = a[lane] - b[lane];
            sums[lane] += apart * apart;
        }
    }
    let rest: f32 = a_rest.iter().zip(b_rest).map(|(a, b)| (a - b) * (a - b)).sum();
    sums.iter().sum::<f32>() + rest
}

/// The length of a vector.
pub fn norm(a: &[f32]) -> f32 {
    dot(a, a).sqrt()
}

/// How alike two vectors are by direction: the cosine of the angle
/// between them. A vector of no length is like nothing: 0.
pub fn similarity(a: &[f32], b: &[f32]) -> f32 {
    let lengths = norm(a) * norm(b);
    if lengths == 0.0 {
        0.0
    } else {
        dot(a, b) / lengths
    }
}

/// How near `row` is to `query` by `metric`: what a search ranks by and
/// gives as `_score`. Higher is nearer in every metric.
pub fn score(metric: Metric, row: &[f32], query: &[f32]) -> f32 {
    match metric {
        Metric::Dot => dot(row, query),
        Metric::L2 => -squared_distance(row, query),
        Metric::Cosine => {
            let lengths = norm(row) * norm(query);
            if lengths == 0.0 {
                0.0
            } else {
                dot(row, query) / lengths
            }
        }
    }
}

/// How a search ranks: by the cosine, by the straight-line distance, or
/// by the dot product. A higher score is nearer in every one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    Cosine,
    L2,
    Dot,
}

/// The `k` rows of `rows` nearest `query`, nearest first, as their
/// position and score. `rows` holds one vector after another, each as
/// long as the query. Rows that score the same keep their order.
pub fn nearest(rows: &[f32], query: &[f32], metric: Metric, k: usize) -> Vec<(usize, f32)> {
    nearest_among(rows, query, metric, k, |_| true)
}

/// [`nearest`], of only the rows `candidate` says are to be compared.
pub fn nearest_among(
    rows: &[f32],
    query: &[f32],
    metric: Metric,
    k: usize,
    candidate: impl Fn(usize) -> bool,
) -> Vec<(usize, f32)> {
    let dimension = query.len();
    if dimension == 0 || k == 0 {
        return Vec::new();
    }
    let query_norm = norm(query);
    let mut scored: Vec<(usize, f32)> = rows
        .chunks_exact(dimension)
        .enumerate()
        .filter(|(position, _)| candidate(*position))
        .map(|(position, row)| (position, match metric {
            Metric::Dot => dot(row, query),
            Metric::L2 => -squared_distance(row, query),
            Metric::Cosine => {
                let lengths = norm(row) * query_norm;
                if lengths == 0.0 {
                    0.0
                } else {
                    dot(row, query) / lengths
                }
            }
        }))
        .collect();
    let nearer = |a: &(usize, f32), b: &(usize, f32)| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0));
    if k < scored.len() {
        scored.select_nth_unstable_by(k, nearer);
        scored.truncate(k);
    }
    scored.sort_by(nearer);
    scored
}

/// Whether `name` is one of the functions here, which take a vector as it
/// is. Every other function is given a vector as the list of its numbers.
pub fn is_vector_function(name: &str) -> bool {
    FUNCTIONS.iter().any(|(known, _)| *known == name)
}

/// The vector functions, by name.
pub const FUNCTIONS: [(&str, fn(&[Value]) -> OQLResult<Value>); 7] = [
    ("dot", |args| of_two("dot", args, |a, b| number(dot(a, b)))),
    ("similarity", |args| of_two("similarity", args, |a, b| number(similarity(a, b)))),
    ("distance", |args| of_two("distance", args, |a, b| number(squared_distance(a, b).sqrt()))),
    ("norm", |args| of_one("norm", args, |a| number(norm(a)))),
    ("normalize", |args| {
        of_one("normalize", args, |a| {
            let length = norm(a);
            Value::Vector(match length == 0.0 {
                true => a.to_vec(),
                false => a.iter().map(|number| number / length).collect(),
            })
        })
    }),
    ("vector", |args| of_one("vector", args, |a| Value::Vector(a.to_vec()))),
    ("centroid", centroid),
];

fn number(value: f32) -> Value {
    Value::Float(f64::from(value))
}

fn failed(message: String) -> OQLError {
    OQLError::FunctionError { message }
}

/// What a value is called when it is not the vector that was wanted.
fn described(value: &Value) -> String {
    match value {
        Value::List(items) if items.is_empty() => "an empty list".to_string(),
        Value::List(_) => "a list that is not all numbers".to_string(),
        other => format!("a {}", other.type_name().to_lowercase()),
    }
}

fn wanted<'a>(name: &str, value: &'a Value) -> OQLResult<Cow<'a, [f32]>> {
    value
        .as_vector()
        .ok_or_else(|| failed(format!("{name}() takes a vector, and was given {}", described(value))))
}

fn of_one(name: &str, args: &[Value], work: impl Fn(&[f32]) -> Value) -> OQLResult<Value> {
    match args {
        [Value::Null] => Ok(Value::Null),
        [one] => Ok(work(&wanted(name, one)?)),
        _ => Err(failed(format!("{name}() takes one vector, and was given {} values", args.len()))),
    }
}

fn of_two(name: &str, args: &[Value], work: impl Fn(&[f32], &[f32]) -> Value) -> OQLResult<Value> {
    match args {
        [Value::Null, _] | [_, Value::Null] => Ok(Value::Null),
        [a, b] => {
            let (a, b) = (wanted(name, a)?, wanted(name, b)?);
            same_length(name, &a, &b)?;
            Ok(work(&a, &b))
        }
        _ => Err(failed(format!("{name}() takes two vectors, and was given {} values", args.len()))),
    }
}

fn same_length(what: &str, a: &[f32], b: &[f32]) -> OQLResult<()> {
    if a.len() == b.len() {
        return Ok(());
    }
    Err(failed(format!(
        "{what} needs two vectors of one length, and these have {} and {} numbers",
        a.len(),
        b.len()
    )))
}

/// The mean of several vectors. Nulls among them are left out; of none,
/// it is null.
fn centroid(args: &[Value]) -> OQLResult<Value> {
    let vectors = match args {
        [Value::Null] => return Ok(Value::Null),
        // One vector is its own mean.
        [one @ Value::Vector(_)] => return Ok(one.clone()),
        [Value::List(vectors)] => vectors,
        _ => {
            return Err(failed(
                "centroid() takes the vectors to average, as in centroid(.documents.embedding)".to_string(),
            ))
        }
    };
    let mut sum: Vec<f64> = Vec::new();
    let mut count = 0usize;
    for value in vectors.iter().filter(|value| !value.is_null()) {
        let vector = wanted("centroid", value)?;
        if count == 0 {
            sum = vec![0.0; vector.len()];
        }
        if vector.len() != sum.len() {
            return Err(failed(format!(
                "centroid() needs vectors of one length, and these have {} and {} numbers",
                sum.len(),
                vector.len()
            )));
        }
        for (total, number) in sum.iter_mut().zip(vector.iter()) {
            *total += f64::from(*number);
        }
        count += 1;
    }
    Ok(match count {
        0 => Value::Null,
        count => Value::Vector(sum.iter().map(|total| (total / count as f64) as f32).collect()),
    })
}

/// Arithmetic with a vector on either side, or `None` when neither side
/// is one, so that lists keep meaning what they did: `+` joins two lists.
pub fn arithmetic(left: &Value, operator: &BinaryOperator, right: &Value) -> Option<OQLResult<Value>> {
    if !matches!(left, Value::Vector(_)) && !matches!(right, Value::Vector(_)) {
        return None;
    }
    let scalar = |value: &Value| match value {
        Value::Integer(number) => Some(*number as f32),
        Value::Float(number) => Some(*number as f32),
        _ => None,
    };
    let symbol = match operator {
        BinaryOperator::Add => "+",
        BinaryOperator::Subtract => "-",
        BinaryOperator::Multiply => "*",
        BinaryOperator::Divide => "/",
        BinaryOperator::Modulo => "%",
    };
    let each = |numbers: &[f32], work: &dyn Fn(f32) -> f32| {
        Ok(Value::Vector(numbers.iter().map(|number| work(*number)).collect()))
    };
    Some(match (left, operator, right) {
        // No vector on one side: there is nothing to work out.
        (Value::Null, _, _) | (_, _, Value::Null) => Ok(Value::Null),
        (Value::Vector(numbers), BinaryOperator::Multiply, by) if scalar(by).is_some() => {
            let by = scalar(by).unwrap_or(1.0);
            each(numbers, &|number| number * by)
        }
        (by, BinaryOperator::Multiply, Value::Vector(numbers)) if scalar(by).is_some() => {
            let by = scalar(by).unwrap_or(1.0);
            each(numbers, &|number| number * by)
        }
        (Value::Vector(numbers), BinaryOperator::Divide, by) if scalar(by).is_some() => {
            match scalar(by).unwrap_or(1.0) {
                by if by == 0.0 => Err(failed("a vector cannot be divided by zero".to_string())),
                by => each(numbers, &|number| number / by),
            }
        }
        (a, BinaryOperator::Add | BinaryOperator::Subtract, b) => match (a.as_vector(), b.as_vector()) {
            (Some(a), Some(b)) => same_length(symbol, &a, &b).map(|()| {
                let adding = matches!(operator, BinaryOperator::Add);
                Value::Vector(
                    a.iter()
                        .zip(b.iter())
                        .map(|(a, b)| if adding { a + b } else { a - b })
                        .collect(),
                )
            }),
            _ => Err(failed(format!(
                "{symbol} between a vector and {} has no meaning; a vector is added to or taken from a vector",
                described(if matches!(a, Value::Vector(_)) { b } else { a })
            ))),
        },
        _ => Err(failed(format!(
            "{symbol} has no meaning here: a vector is added to or taken from a vector, and multiplied or divided by a number"
        ))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kernels_agree_with_the_sums_written_out() {
        // Lengths either side of the eight the loops go by.
        for length in [1usize, 7, 8, 9, 16, 31, 384] {
            let a: Vec<f32> = (0..length).map(|n| (n as f32 * 0.37).sin()).collect();
            let b: Vec<f32> = (0..length).map(|n| (n as f32 * 0.11).cos()).collect();
            let plain_dot: f64 = a.iter().zip(&b).map(|(a, b)| f64::from(*a) * f64::from(*b)).sum();
            let plain_apart: f64 = a.iter().zip(&b).map(|(a, b)| f64::from(a - b).powi(2)).sum();
            assert!((f64::from(dot(&a, &b)) - plain_dot).abs() < 1e-4, "dot at {length}");
            assert!((f64::from(squared_distance(&a, &b)) - plain_apart).abs() < 1e-4, "distance at {length}");
        }
        assert_eq!(similarity(&[1.0, 0.0], &[2.0, 0.0]), 1.0);
        assert_eq!(similarity(&[1.0, 0.0], &[0.0, 3.0]), 0.0);
        assert_eq!(similarity(&[1.0, 0.0], &[0.0, 0.0]), 0.0);
    }

    #[test]
    fn nearest_gives_the_best_first_and_keeps_ties_in_order() {
        let rows = [1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.9, 0.1, -1.0, 0.0];
        let hits: Vec<usize> = nearest(&rows, &[1.0, 0.0], Metric::Cosine, 3).iter().map(|hit| hit.0).collect();
        assert_eq!(hits, [0, 2, 3]);
        assert_eq!(nearest(&rows, &[1.0, 0.0], Metric::Cosine, 99).len(), 5);
        let nearest_point: Vec<usize> = nearest(&rows, &[0.0, 1.0], Metric::L2, 1).iter().map(|hit| hit.0).collect();
        assert_eq!(nearest_point, [1]);
        assert!(nearest(&rows, &[1.0, 0.0], Metric::Dot, 0).is_empty());
    }
}
