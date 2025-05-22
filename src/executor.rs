use crate::ast::*;
use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use sled::{Batch, Db};
use std::collections::{HashMap, VecDeque};
use std::error::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Row {
    data: HashMap<String, String>,
}

pub fn execute(
    program: Program,
    db: &Db,
) -> Result<HashMap<String, Vec<HashMap<String, String>>>, Box<dyn Error>> {
    let schemas_tree = db.open_tree("schemas")?;
    let queries_tree = db.open_tree("queries")?;

    let mut schemas: HashMap<String, HashMap<String, FieldType>> = HashMap::new();
    for schema in &program.schemas {
        schemas.insert(schema.name.clone(), schema.fields.clone());
        let schema_key = format!("schema_{}", schema.name);
        let schema_value = serde_json::to_vec(&schema.fields)?;
        schemas_tree.insert(schema_key.as_bytes(), schema_value)?;
    }
    schemas_tree.flush()?;

    let mut functions: HashMap<String, Function> = HashMap::new();
    for func in &program.functions {
        functions.insert(func.name.clone(), (*func).clone());
    }

    fn resolve_collection(
        source: &QuerySource,
        program: &Program,
        schemas: &HashMap<String, HashMap<String, FieldType>>,
        db: &Db,
        query_results: &HashMap<String, VecDeque<Row>>,
        queries_tree: &sled::Tree,
    ) -> Result<(String, String, sled::Tree, VecDeque<Row>), String> {
        match source {
            QuerySource::Collection(coll_name) => {
                if schemas.contains_key(coll_name) {
                    let coll = db.open_tree(coll_name).map_err(|e| e.to_string())?;
                    let mut result = VecDeque::new();
                    for item in coll.iter() {
                        let (_, value) = item.map_err(|e| e.to_string())?;
                        let row: Row = serde_json::from_slice(&value).map_err(|e| e.to_string())?;
                        result.push_back(row);
                    }
                    Ok((coll_name.clone(), coll_name.clone(), coll, result))
                } else {
                    let source_query = program
                        .queries
                        .iter()
                        .find(|q| q.name == *coll_name)
                        .ok_or(format!("No query or schema found for '{}'", coll_name))?;
                    resolve_collection(
                        &source_query.source,
                        program,
                        schemas,
                        db,
                        query_results,
                        queries_tree,
                    )
                    .map(|(cn, bsn, coll, mut res)| {
                        if let Some(qr) = query_results.get(coll_name) {
                            res = qr.clone();
                        } else if let Ok(Some(data)) =
                            queries_tree.get(format!("result_{}", coll_name).as_bytes())
                        {
                            res = serde_json::from_slice(&data).unwrap();
                        }
                        (coll_name.clone(), bsn, coll, res)
                    })
                }
            }
            QuerySource::Query(query_name) => {
                let source_query = program
                    .queries
                    .iter()
                    .find(|q| q.name == *query_name)
                    .ok_or(format!("No query found for '{}'", query_name))?;
                resolve_collection(
                    &source_query.source,
                    program,
                    schemas,
                    db,
                    query_results,
                    queries_tree,
                )
                .map(|(cn, bsn, coll, mut res)| {
                    if let Some(qr) = query_results.get(query_name) {
                        res = qr.clone();
                    } else if let Ok(Some(data)) =
                        queries_tree.get(format!("result_{}", query_name).as_bytes())
                    {
                        res = serde_json::from_slice(&data).unwrap();
                    }
                    (query_name.clone(), bsn, coll, res)
                })
            }
        }
    }

    let mut query_results: HashMap<String, VecDeque<Row>> = HashMap::new();
    for query in &program.queries {
        let (collection_name, base_schema_name, collection, initial_result) = resolve_collection(
            &query.source,
            &program,
            &schemas,
            db,
            &query_results,
            &queries_tree,
        )?;

        let schema = schemas.get(&base_schema_name).ok_or(format!(
            "Schema not found for base collection '{}'",
            base_schema_name
        ))?;

        let mut result = if query
            .operators
            .iter()
            .any(|op| matches!(op, Operator::Insert { .. } | Operator::BatchInsert { .. }))
        {
            VecDeque::new()
        } else {
            initial_result
        };

        for operator in &query.operators {
            match operator {
                Operator::Count { field: _ } => {
                    let count = result.len();
                    let mut new_result = VecDeque::new();
                    let mut row_data = HashMap::new();
                    row_data.insert("count".to_string(), count.to_string());
                    new_result.push_back(Row { data: row_data });
                    result = new_result;
                }
                Operator::Avg { name, field } => {
                    let values: Vec<f64> = result
                        .iter()
                        .filter_map(|row| {
                            row.data.get(&field.join(".")).and_then(|v| v.parse().ok())
                        })
                        .collect();
                    let avg = if values.is_empty() {
                        0.0
                    } else {
                        values.iter().sum::<f64>() / values.len() as f64
                    };
                    let mut new_result = VecDeque::new();
                    let mut row_data = HashMap::new();
                    row_data.insert(name.clone(), avg.to_string());
                    new_result.push_back(Row { data: row_data });
                    result = new_result;
                }
                Operator::Max { name, field } => {
                    let max = result
                        .iter()
                        .filter_map(|row| {
                            row.data
                                .get(&field.join("."))
                                .and_then(|v| v.parse::<f64>().ok())
                        })
                        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    let mut new_result = VecDeque::new();
                    let mut row_data = HashMap::new();
                    row_data.insert(name.clone(), max.map_or("".to_string(), |v| v.to_string()));
                    new_result.push_back(Row { data: row_data });
                    result = new_result;
                }
                Operator::Min { name, field } => {
                    let min = result
                        .iter()
                        .filter_map(|row| {
                            row.data
                                .get(&field.join("."))
                                .and_then(|v| v.parse::<f64>().ok())
                        })
                        .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    let mut new_result = VecDeque::new();
                    let mut row_data = HashMap::new();
                    row_data.insert(name.clone(), min.map_or("".to_string(), |v| v.to_string()));
                    new_result.push_back(Row { data: row_data });
                    result = new_result;
                }
                Operator::GeoWithin { field, bounds } => {
                    result.retain(|row| {
                        if let Some(value) = row.data.get(&field.join(".")) {
                            if let Ok(geo) = serde_json::from_str::<GeoPointLiteral>(value) {
                                geo.lat >= bounds.min_lat
                                    && geo.lat <= bounds.max_lat
                                    && geo.lon >= bounds.min_lon
                                    && geo.lon <= bounds.max_lon
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    });
                }
                Operator::GeoDistance {
                    name,
                    field,
                    point,
                    max_distance,
                } => {
                    let mut new_result = VecDeque::new();
                    for mut row in result.drain(..) {
                        if let Some(value) = row.data.get(&field.join(".")) {
                            if let Ok(geo) = serde_json::from_str::<GeoPointLiteral>(value) {
                                let distance = haversine_distance(&geo, point);
                                if distance <= *max_distance {
                                    row.data.insert(name.clone(), distance.to_string());
                                    new_result.push_back(row);
                                }
                            }
                        }
                    }
                    result = new_result;
                }
                Operator::Lead {
                    name,
                    field,
                    offset,
                    partition,
                    order,
                } => {
                    let mut partitioned = HashMap::new();
                    if let Some(partition_fields) = partition {
                        for row in result.clone() {
                            let key = partition_fields
                                .iter()
                                .map(|field| {
                                    row.data
                                        .get(&field.join("."))
                                        .unwrap_or(&"".to_string())
                                        .clone()
                                })
                                .collect::<Vec<_>>()
                                .join("|");
                            partitioned.entry(key).or_insert_with(Vec::new).push(row);
                        }
                    } else {
                        partitioned.insert("all".to_string(), result.clone().into_iter().collect());
                    }
                    let mut new_result = VecDeque::new();
                    for (_, mut rows) in partitioned {
                        rows.sort_by(|a, b| {
                            order
                                .iter()
                                .fold(std::cmp::Ordering::Equal, |acc, (expr, dir)| {
                                    let a_val = evaluate_expr_with_db(
                                        expr,
                                        a,
                                        db,
                                        &base_schema_name,
                                        Some(schema),
                                        &functions,
                                    )
                                    .unwrap_or_default();
                                    let b_val = evaluate_expr_with_db(
                                        expr,
                                        b,
                                        db,
                                        &base_schema_name,
                                        Some(schema),
                                        &functions,
                                    )
                                    .unwrap_or_default();
                                    let cmp = a_val.cmp(&b_val);
                                    match dir {
                                        Direction::Asc => acc.then(cmp),
                                        Direction::Desc => acc.then(cmp.reverse()),
                                    }
                                })
                        });
                        let rows_clone = rows.clone();
                        for i in 0..rows.len() {
                            let lead_idx = (i as i32 + offset) as usize;
                            let lead_value = if lead_idx < rows_clone.len() {
                                rows_clone[lead_idx].data.get(&field.join(".")).cloned()
                            } else {
                                None
                            };
                            rows[i]
                                .data
                                .insert(name.clone(), lead_value.unwrap_or("null".to_string()));
                            new_result.push_back(rows[i].clone());
                        }
                    }
                    result = new_result;
                }
                Operator::Lag {
                    name,
                    field,
                    offset,
                    partition,
                    order,
                } => {
                    let mut partitioned = HashMap::new();
                    if let Some(partition_fields) = partition {
                        for row in result.clone() {
                            let key = partition_fields
                                .iter()
                                .map(|field| {
                                    row.data
                                        .get(&field.join("."))
                                        .unwrap_or(&"".to_string())
                                        .clone()
                                })
                                .collect::<Vec<_>>()
                                .join("|");
                            partitioned.entry(key).or_insert_with(Vec::new).push(row);
                        }
                    } else {
                        partitioned.insert("all".to_string(), result.clone().into_iter().collect());
                    }
                    let mut new_result = VecDeque::new();
                    for (_, mut rows) in partitioned {
                        rows.sort_by(|a, b| {
                            order
                                .iter()
                                .fold(std::cmp::Ordering::Equal, |acc, (expr, dir)| {
                                    let a_val = evaluate_expr_with_db(
                                        expr,
                                        a,
                                        db,
                                        &base_schema_name,
                                        Some(schema),
                                        &functions,
                                    )
                                    .unwrap_or_default();
                                    let b_val = evaluate_expr_with_db(
                                        expr,
                                        b,
                                        db,
                                        &base_schema_name,
                                        Some(schema),
                                        &functions,
                                    )
                                    .unwrap_or_default();
                                    let cmp = a_val.cmp(&b_val);
                                    match dir {
                                        Direction::Asc => acc.then(cmp),
                                        Direction::Desc => acc.then(cmp.reverse()),
                                    }
                                })
                        });
                        let rows_clone = rows.clone();
                        for i in 0..rows.len() {
                            let lag_idx = i as i32 - offset;
                            let lag_value = if lag_idx >= 0 && (lag_idx as usize) < rows_clone.len()
                            {
                                rows_clone[lag_idx as usize]
                                    .data
                                    .get(&field.join("."))
                                    .cloned()
                            } else {
                                None
                            };
                            rows[i]
                                .data
                                .insert(name.clone(), lag_value.unwrap_or("null".to_string()));
                            new_result.push_back(rows[i].clone());
                        }
                    }
                    result = new_result;
                }
                Operator::Insert { values } => {
                    let mut row_data = HashMap::new();
                    for (key, expr) in values {
                        if !schema.contains_key(key) {
                            return Err(format!(
                                "Field '{}' not in schema for '{}'",
                                key, base_schema_name
                            )
                            .into());
                        }
                        let value = match expr {
                            Expr::Literal(Literal::String(s)) => match schema[key] {
                                FieldType::String | FieldType::DateTime => s.clone(),
                                FieldType::Int8 | FieldType::Float => {
                                    s.parse::<f64>().map(|n| n.to_string()).map_err(|_| {
                                        format!("Field '{}' expects number, got '{}'", key, s)
                                    })?
                                }
                                FieldType::Bool => {
                                    if s == "true" || s == "false" {
                                        s.clone()
                                    } else {
                                        return Err(format!(
                                            "Field '{}' expects bool, got '{}'",
                                            key, s
                                        )
                                        .into());
                                    }
                                }
                                FieldType::Ref(_) => s.clone(),
                                _ => return Err(format!("Field '{}' type mismatch", key).into()),
                            },
                            Expr::Literal(Literal::Number(n)) => {
                                if matches!(schema[key], FieldType::Int8 | FieldType::Float) {
                                    n.to_string()
                                } else {
                                    return Err(format!(
                                        "Field '{}' expects {:?}, got number",
                                        key, schema[key]
                                    )
                                    .into());
                                }
                            }
                            Expr::Literal(Literal::Bool(b)) => {
                                if matches!(schema[key], FieldType::Bool) {
                                    b.to_string()
                                } else {
                                    return Err(
                                        format!("Field '{}' expects Bool, got bool", key).into()
                                    );
                                }
                            }
                            Expr::Literal(Literal::Null) => "null".to_string(),
                            _ => evaluate_expr_with_db(
                                expr,
                                &Row {
                                    data: HashMap::new(),
                                },
                                db,
                                &base_schema_name,
                                Some(schema),
                                &functions,
                            )?,
                        };
                        row_data.insert(key.clone(), value);
                    }
                    let id = row_data
                        .get("id")
                        .ok_or("Missing 'id' field in insert")?
                        .clone();
                    let row = Row {
                        data: row_data.clone(),
                    };
                    let value = serde_json::to_vec(&row)?;
                    collection.insert(id.as_bytes(), value)?;
                    collection.flush()?;
                    result.push_back(row);
                }
                Operator::BatchInsert { values } => {
                    let mut batch = Batch::default();
                    for row_values in values {
                        let mut row_data = HashMap::new();
                        for (key, expr) in row_values {
                            if !schema.contains_key(key) {
                                return Err(format!(
                                    "Field '{}' not in schema for '{}'",
                                    key, base_schema_name
                                )
                                .into());
                            }
                            let field_type = schema.get(key).unwrap();
                            let value = match (field_type, expr) {
                                (
                                    FieldType::GeoPoint { .. },
                                    Expr::Literal(Literal::Object(obj)),
                                ) => {
                                    let lat = obj
                                        .get("lat")
                                        .and_then(|e| {
                                            if let Expr::Literal(Literal::Number(n)) = e {
                                                Some(*n)
                                            } else {
                                                None
                                            }
                                        })
                                        .ok_or("Missing or invalid 'lat' in GeoPoint")?;
                                    let lon = obj
                                        .get("lon")
                                        .and_then(|e| {
                                            if let Expr::Literal(Literal::Number(n)) = e {
                                                Some(*n)
                                            } else {
                                                None
                                            }
                                        })
                                        .ok_or("Missing or invalid 'lon' in GeoPoint")?;
                                    let geo = GeoPointLiteral { lat, lon };
                                    serde_json::to_string(&geo)?
                                }
                                (_, expr) => evaluate_expr_with_db(
                                    expr,
                                    &Row {
                                        data: HashMap::new(),
                                    },
                                    db,
                                    &base_schema_name,
                                    Some(schema),
                                    &functions,
                                )?,
                            };
                            row_data.insert(key.clone(), value);
                        }
                        let id = row_data
                            .get("id")
                            .ok_or("Missing 'id' field in batch insert")?
                            .clone();
                        let row = Row {
                            data: row_data.clone(),
                        };
                        let value = serde_json::to_vec(&row)?;
                        batch.insert(id.as_bytes(), value);
                        result.push_back(row);
                    }
                    collection.apply_batch(batch)?;
                    collection.flush()?;
                }
                Operator::Filter { condition } => {
                    result.retain(|row| {
                        evaluate_condition(
                            condition,
                            row,
                            db,
                            &base_schema_name,
                            Some(schema),
                            &functions,
                        )
                        .unwrap_or(false)
                    });
                }
                Operator::Select { fields } => {
                    let all_rows: Vec<Row> = result.clone().into_iter().collect();
                    let mut new_result = VecDeque::new();
                    for row in result.drain(..) {
                        let mut new_row = HashMap::new();
                        for field in fields {
                            let value = match &field.expr {
                                Expr::Function { name, args } if name == "count" => {
                                    if args.len() != 1 {
                                        return Err("count expects exactly one argument".into());
                                    }
                                    let field_path = match &args[0] {
                                        Expr::FieldPath(path) => path.join("."),
                                        _ => {
                                            return Err("count argument must be a field path".into())
                                        }
                                    };
                                    let count = all_rows
                                        .iter()
                                        .filter(|r| r.data.contains_key(&field_path))
                                        .count();
                                    count.to_string()
                                }
                                Expr::FieldPath(path) if path.len() > 1 => {
                                    let mut current_value =
                                        row.data.get(&path[0]).cloned().unwrap_or_default();
                                    let mut current_schema = Some(schema);
                                    let mut current_field = &path[0];
                                    for segment in &path[1..] {
                                        if let Some(cs) = current_schema {
                                            if let Some(FieldType::Ref(ref_coll)) =
                                                cs.get(current_field)
                                            {
                                                let ref_tree = db.open_tree(ref_coll)?;
                                                if let Ok(Some(ref_data)) =
                                                    ref_tree.get(current_value.as_bytes())
                                                {
                                                    let ref_row: Row =
                                                        serde_json::from_slice(&ref_data)?;
                                                    current_value = ref_row
                                                        .data
                                                        .get(segment)
                                                        .cloned()
                                                        .unwrap_or_default();
                                                    current_field = segment;
                                                    current_schema = schemas.get(ref_coll);
                                                } else {
                                                    current_value = "".to_string();
                                                    break;
                                                }
                                            } else {
                                                current_value = "".to_string();
                                                break;
                                            }
                                        } else {
                                            current_value = "".to_string();
                                            break;
                                        }
                                    }
                                    current_value
                                }
                                _ => evaluate_group_expr(
                                    &field.expr,
                                    &[row.clone()],
                                    db,
                                    &base_schema_name,
                                    Some(schema),
                                    &functions,
                                )?,
                            };
                            new_row.insert(field.name.clone(), value);
                        }
                        new_result.push_back(Row { data: new_row });
                    }
                    result = new_result;
                }
                Operator::Sort { fields } => {
                    let mut rows_vec: Vec<Row> = result.into_iter().collect();
                    rows_vec.sort_by(|a, b| {
                        fields
                            .iter()
                            .fold(std::cmp::Ordering::Equal, |acc, (expr, dir)| {
                                let a_val = evaluate_expr_with_db(
                                    expr,
                                    a,
                                    db,
                                    &base_schema_name,
                                    Some(schema),
                                    &functions,
                                )
                                .unwrap_or_default();
                                let b_val = evaluate_expr_with_db(
                                    expr,
                                    b,
                                    db,
                                    &base_schema_name,
                                    Some(schema),
                                    &functions,
                                )
                                .unwrap_or_default();
                                let cmp = a_val.cmp(&b_val);
                                match dir {
                                    Direction::Asc => acc.then(cmp),
                                    Direction::Desc => acc.then(cmp.reverse()),
                                }
                            })
                    });
                    result = VecDeque::from(rows_vec);
                }
                Operator::Limit { count } => {
                    result.truncate(*count);
                }
                Operator::Skip { count } => {
                    result.drain(0..*count.min(&result.len()));
                }
                Operator::Accumulate {
                    name,
                    field,
                    partition,
                } => {
                    let mut partitioned = HashMap::new();
                    if let Some(partition_fields) = partition {
                        for row in result.clone() {
                            let key = partition_fields
                                .iter()
                                .map(|field| {
                                    row.data
                                        .get(&field.join("."))
                                        .unwrap_or(&"".to_string())
                                        .clone()
                                })
                                .collect::<Vec<_>>()
                                .join("|");
                            partitioned.entry(key).or_insert_with(Vec::new).push(row);
                        }
                    } else {
                        partitioned.insert("all".to_string(), result.clone().into_iter().collect());
                    }
                    let mut new_result = VecDeque::new();
                    for (_, mut rows) in partitioned {
                        let mut running_total = 0.0;
                        for row in rows.iter_mut() {
                            let value = evaluate_expr_with_db(
                                field,
                                row,
                                db,
                                &base_schema_name,
                                Some(schema),
                                &functions,
                            )?
                            .parse::<f64>()?;
                            running_total += value;
                            row.data.insert(name.clone(), running_total.to_string());
                            new_result.push_back(row.clone());
                        }
                    }
                    result = new_result;
                }
                Operator::TimeWindow {
                    name,
                    field,
                    window,
                    date_field,
                    partition,
                } => {
                    let window_parts: Vec<&str> = window.split_whitespace().collect();
                    if window_parts.len() != 2 || window_parts[1] != "days" {
                        return Err("Window must be in format 'X days'".into());
                    }
                    let days: i64 = window_parts[0].parse()?;
                    let window_duration = Duration::days(days);
                    let mut partitioned = HashMap::new();
                    if let Some(partition_fields) = partition {
                        for row in result.clone() {
                            let key = partition_fields
                                .iter()
                                .map(|field| {
                                    row.data
                                        .get(&field.join("."))
                                        .unwrap_or(&"".to_string())
                                        .clone()
                                })
                                .collect::<Vec<_>>()
                                .join("|");
                            partitioned.entry(key).or_insert_with(Vec::new).push(row);
                        }
                    } else {
                        partitioned.insert("all".to_string(), result.clone().into_iter().collect());
                    }
                    let mut new_result = VecDeque::new();
                    for (_, rows) in partitioned {
                        let mut rows_vec: Vec<Row> = rows.into_iter().collect();
                        rows_vec.sort_by(|a, b| {
                            let a_date = NaiveDate::parse_from_str(&a.data[date_field], "%Y-%m-%d")
                                .unwrap_or_default();
                            let b_date = NaiveDate::parse_from_str(&b.data[date_field], "%Y-%m-%d")
                                .unwrap_or_default();
                            a_date.cmp(&b_date)
                        });
                        let mut window_sums = Vec::new();
                        for (i, row) in rows_vec.iter().enumerate() {
                            let current_date =
                                NaiveDate::parse_from_str(&row.data[date_field], "%Y-%m-%d")
                                    .map_err(|_| "Invalid date format, expected YYYY-MM-DD")?;
                            let window_start = current_date - window_duration;
                            let mut window_sum = 0.0;
                            for prev_row in rows_vec.iter().take(i + 1) {
                                let prev_date = NaiveDate::parse_from_str(
                                    &prev_row.data[date_field],
                                    "%Y-%m-%d",
                                )
                                .map_err(|_| "Invalid date format, expected YYYY-MM-DD")?;
                                if prev_date >= window_start && prev_date <= current_date {
                                    let value = evaluate_expr_with_db(
                                        field,
                                        prev_row,
                                        db,
                                        &base_schema_name,
                                        Some(schema),
                                        &functions,
                                    )?
                                    .parse::<f64>()?;
                                    window_sum += value;
                                }
                            }
                            window_sums.push(window_sum);
                        }
                        for (i, mut row) in rows_vec.into_iter().enumerate() {
                            row.data.insert(name.clone(), window_sums[i].to_string());
                            new_result.push_back(row);
                        }
                    }
                    result = new_result;
                }
                Operator::Group { by, alias } => {
                    let mut grouped = HashMap::new();
                    for row in result.drain(..) {
                        let key = evaluate_expr_with_db(
                            by,
                            &row,
                            db,
                            &base_schema_name,
                            Some(schema),
                            &functions,
                        )?;
                        grouped.entry(key).or_insert_with(Vec::new).push(row);
                    }
                    let mut new_result = VecDeque::new();
                    for (key, rows) in grouped {
                        let mut new_row = HashMap::new();
                        new_row.insert(alias.clone(), key);
                        new_result.push_back(Row { data: new_row });
                    }
                    result = new_result;
                }
                Operator::Rank {
                    name,
                    partition,
                    order,
                } => {
                    let mut partitioned = HashMap::new();
                    if let Some(partition_fields) = partition {
                        for row in result.clone() {
                            let key = partition_fields
                                .iter()
                                .map(|field| {
                                    row.data
                                        .get(&field.join("."))
                                        .unwrap_or(&"".to_string())
                                        .clone()
                                })
                                .collect::<Vec<_>>()
                                .join("|");
                            partitioned.entry(key).or_insert_with(Vec::new).push(row);
                        }
                    } else {
                        partitioned.insert("all".to_string(), result.clone().into_iter().collect());
                    }
                    let mut new_result = VecDeque::new();
                    for (_, mut rows) in partitioned {
                        rows.sort_by(|a, b| {
                            order
                                .iter()
                                .fold(std::cmp::Ordering::Equal, |acc, (expr, dir)| {
                                    let a_val = evaluate_expr_with_db(
                                        expr,
                                        a,
                                        db,
                                        &base_schema_name,
                                        Some(schema),
                                        &functions,
                                    )
                                    .unwrap_or_default();
                                    let b_val = evaluate_expr_with_db(
                                        expr,
                                        b,
                                        db,
                                        &base_schema_name,
                                        Some(schema),
                                        &functions,
                                    )
                                    .unwrap_or_default();
                                    let cmp = a_val.cmp(&b_val);
                                    match dir {
                                        Direction::Asc => acc.then(cmp),
                                        Direction::Desc => acc.then(cmp.reverse()),
                                    }
                                })
                        });
                        for (i, row) in rows.iter_mut().enumerate() {
                            row.data.insert(name.clone(), (i + 1).to_string());
                            new_result.push_back(row.clone());
                        }
                    }
                    result = new_result;
                }
                Operator::Sum { field } => {
                    let sum: f64 = result
                        .iter()
                        .filter_map(|row| {
                            row.data
                                .get(&field.join("."))
                                .and_then(|v| v.parse::<f64>().ok())
                        })
                        .sum();
                    let mut new_result = VecDeque::new();
                    let mut row_data = HashMap::new();
                    row_data.insert("sum".to_string(), sum.to_string());
                    new_result.push_back(Row { data: row_data });
                    result = new_result;
                }
            }
        }

        let query_key = format!("result_{}", query.name);
        let result_value = serde_json::to_vec(&result)?;
        queries_tree.insert(query_key.as_bytes(), result_value)?;
        queries_tree.flush()?;
        query_results.insert(query.name.clone(), result);
    }

    let final_result = match program.result {
        Some(result_map) => {
            let mut output = HashMap::new();
            for (key, query_name) in result_map {
                let rows = query_results
                    .get(&query_name)
                    .ok_or(format!("Query '{}' not found for result", query_name))?
                    .iter()
                    .map(|row| row.data.clone())
                    .collect::<Vec<_>>();
                output.insert(key, rows);
            }
            output
        }
        None => query_results
            .into_iter()
            .last()
            .map(|(_, rows)| {
                let mut map = HashMap::new();
                map.insert(
                    "result".to_string(),
                    rows.into_iter().map(|row| row.data).collect(),
                );
                map
            })
            .unwrap_or_default(),
    };

    Ok(final_result)
}

fn evaluate_expr_with_db(
    expr: &Expr,
    row: &Row,
    db: &Db,
    base_collection: &str,
    schema: Option<&HashMap<String, FieldType>>,
    functions: &HashMap<String, Function>,
) -> Result<String, Box<dyn Error>> {
    match expr {
        Expr::FieldPath(path) => {
            let mut current_row = row.clone();
            let mut current_collection = base_collection.to_string();
            let mut value = None;
            for (i, part) in path.iter().enumerate() {
                if i == 0 {
                    value = current_row.data.get(part).cloned();
                } else if let Some(ref_id) = value {
                    let schema_key = format!("schema_{}", current_collection);
                    let schemas_tree = db.open_tree("schemas")?;
                    let schema_data = if i == 1 && current_collection == base_collection {
                        schema
                            .cloned()
                            .ok_or(format!("No in-memory schema for '{}'", current_collection))?
                    } else {
                        let schema_bytes = schemas_tree
                            .get(schema_key.as_bytes())?
                            .ok_or(format!("Schema not found for '{}'", current_collection))?;
                        serde_json::from_slice(&schema_bytes)?
                    };
                    if let Some(FieldType::Ref(ref_collection)) = schema_data.get(&path[i - 1]) {
                        current_collection = ref_collection.clone();
                        let ref_tree = db.open_tree(&current_collection)?;
                        let ref_data = ref_tree.get(ref_id.as_bytes())?.ok_or(format!(
                            "Ref ID '{}' not found in '{}'",
                            ref_id, current_collection
                        ))?;
                        current_row = serde_json::from_slice(&ref_data)?;
                        value = current_row.data.get(part).cloned();
                    } else {
                        return Err(format!(
                            "Field '{}' in '{}' is not a reference",
                            path[i - 1],
                            current_collection
                        )
                        .into());
                    }
                } else {
                    return Err(format!("No reference ID for '{}'", path[i - 1]).into());
                }
            }
            Ok(value.unwrap_or("".to_string()))
        }
        Expr::Function { name, args } => {
            if let Some(func) = functions.get(name) {
                let mut param_values = HashMap::new();
                for ((param_name, _), arg) in func.params.iter().zip(args.iter()) {
                    param_values.insert(
                        param_name.clone(),
                        evaluate_expr_with_db(arg, row, db, base_collection, schema, functions)?,
                    );
                }
                match &func.body {
                    FunctionBody::Expr(expr) => evaluate_expr_with_db(
                        &expr,
                        &Row { data: param_values },
                        db,
                        base_collection,
                        schema,
                        functions,
                    ),
                    FunctionBody::Query(operators) => {
                        // Execute query operators on a temporary result set
                        unimplemented!("Query function execution not yet supported")
                    }
                }
            } else {
                match name.as_str() {
                    "count" => {
                        if args.len() != 1 {
                            return Err("count expects exactly one argument".into());
                        }
                        let field = match &args[0] {
                            Expr::FieldPath(path) => path.join("."),
                            _ => return Err("count argument must be a field path".into()),
                        };
                        Ok(row
                            .data
                            .contains_key(&field)
                            .then(|| "1")
                            .unwrap_or("0")
                            .to_string())
                    }
                    "sum" => {
                        if args.len() != 1 {
                            return Err("sum expects exactly one argument".into());
                        }
                        let field = match &args[0] {
                            Expr::FieldPath(path) => path.join("."),
                            _ => return Err("sum argument must be a field path".into()),
                        };
                        Ok(row
                            .data
                            .get(&field)
                            .and_then(|v| v.parse::<f64>().ok())
                            .unwrap_or(0.0)
                            .to_string())
                    }
                    "geo.grid_cell" => {
                        if args.len() != 2 {
                            return Err("geo.grid_cell expects two arguments".into());
                        }
                        let coords = evaluate_expr_with_db(
                            &args[0],
                            row,
                            db,
                            base_collection,
                            schema,
                            functions,
                        )?;
                        let resolution = if let Expr::Literal(Literal::String(res)) = &args[1] {
                            res
                        } else {
                            return Err("Expected resolution string".into());
                        };
                        let geo: GeoPointLiteral = serde_json::from_str(&coords)?;
                        let grid_x = (geo.lat / resolution.parse::<f64>()?).floor();
                        let grid_y = (geo.lon / resolution.parse::<f64>()?).floor();
                        Ok(format!("{}_{}", grid_x, grid_y))
                    }
                    _ => Err(format!("Unknown function '{}'", name).into()),
                }
            }
        }
        Expr::Literal(literal) => match literal {
            Literal::Number(n) => Ok(n.to_string()),
            Literal::String(s) => Ok(s.clone()),
            Literal::Bool(b) => Ok(b.to_string()),
            Literal::Null => Ok("null".to_string()),
            Literal::GeoPoint(geo) => Ok(serde_json::to_string(geo)?),
            Literal::Object(obj) => {
                let mut map = HashMap::new();
                for (k, v) in obj {
                    map.insert(
                        k.clone(),
                        evaluate_expr_with_db(v, row, db, base_collection, schema, functions)?,
                    );
                }
                Ok(serde_json::to_string(&map)?)
            }
        },
        Expr::Arithmetic { left, op, right } => {
            let l = evaluate_expr_with_db(left, row, db, base_collection, schema, functions)?
                .parse::<f64>()?;
            let r = evaluate_expr_with_db(right, row, db, base_collection, schema, functions)?
                .parse::<f64>()?;
            match op.as_str() {
                "+" => Ok((l + r).to_string()),
                "-" => Ok((l - r).to_string()),
                "*" => Ok((l * r).to_string()),
                "/" => Ok((l / r).to_string()),
                _ => Err(format!("Unknown operator '{}'", op).into()),
            }
        }
        Expr::Case {
            conditions,
            default,
        } => {
            for (cond, expr) in conditions {
                if evaluate_condition(cond, row, db, base_collection, schema, functions)? {
                    return evaluate_expr_with_db(
                        expr,
                        row,
                        db,
                        base_collection,
                        schema,
                        functions,
                    );
                }
            }
            default.as_ref().map_or(Ok("null".to_string()), |def| {
                evaluate_expr_with_db(def, row, db, base_collection, schema, functions)
            })
        }
        Expr::Lag {
            expr,
            partition,
            order,
        } => {
            let mut partitioned = HashMap::new();
            if let Some(partition_fields) = partition {
                for (i, r) in rows.iter().enumerate() {
                    let key = partition_fields
                        .iter()
                        .map(|field| {
                            r.data
                                .get(&field.join("."))
                                .unwrap_or(&"".to_string())
                                .clone()
                        })
                        .collect::<Vec<_>>()
                        .join("|");
                    partitioned
                        .entry(key)
                        .or_insert_with(Vec::new)
                        .push((i, r.clone()));
                }
            } else {
                partitioned.insert(
                    "all".to_string(),
                    rows.iter()
                        .enumerate()
                        .map(|(i, r)| (i, r.clone()))
                        .collect(),
                );
            }

            let current_key = partition
                .as_ref()
                .map(|fields| {
                    fields
                        .iter()
                        .map(|field| {
                            row.data
                                .get(&field.join("."))
                                .unwrap_or(&"".to_string())
                                .clone()
                        })
                        .collect::<Vec<_>>()
                        .join("|")
                })
                .unwrap_or("all".to_string());

            if let Some(group) = partitioned.get(&current_key) {
                let mut group_vec: Vec<(usize, Row)> = group.clone();
                group_vec.sort_by(|(i_a, a), (i_b, b)| {
                    order
                        .iter()
                        .fold(std::cmp::Ordering::Equal, |acc, (ord_expr, dir)| {
                            let a_val = evaluate_expr_with_db(
                                ord_expr,
                                a,
                                rows,
                                *i_a,
                                db,
                                base_collection,
                                schema,
                                functions,
                            )
                            .unwrap_or_default();
                            let b_val = evaluate_expr_with_db(
                                ord_expr,
                                b,
                                rows,
                                *i_b,
                                db,
                                base_collection,
                                schema,
                                functions,
                            )
                            .unwrap_or_default();
                            let cmp = a_val.cmp(&b_val);
                            match dir {
                                Direction::Asc => acc.then(cmp),
                                Direction::Desc => acc.then(cmp.reverse()),
                            }
                        })
                });

                let pos = group_vec
                    .iter()
                    .position(|(i, _)| *i == row_idx)
                    .unwrap_or(0);
                let lag_idx = if pos > 0 {
                    pos - 1
                } else {
                    return Ok("null".to_string());
                };
                let lag_row = &group_vec[lag_idx].1;
                evaluate_expr_with_db(
                    expr,
                    lag_row,
                    rows,
                    group_vec[lag_idx].0,
                    db,
                    base_collection,
                    schema,
                    functions,
                )
            } else {
                Ok("null".to_string())
            }
        }
        Expr::Lead {
            expr,
            partition,
            order,
        } => {
            let mut partitioned = HashMap::new();
            if let Some(partition_fields) = partition {
                for (i, r) in rows.iter().enumerate() {
                    let key = partition_fields
                        .iter()
                        .map(|field| {
                            r.data
                                .get(&field.join("."))
                                .unwrap_or(&"".to_string())
                                .clone()
                        })
                        .collect::<Vec<_>>()
                        .join("|");
                    partitioned
                        .entry(key)
                        .or_insert_with(Vec::new)
                        .push((i, r.clone()));
                }
            } else {
                partitioned.insert(
                    "all".to_string(),
                    rows.iter()
                        .enumerate()
                        .map(|(i, r)| (i, r.clone()))
                        .collect(),
                );
            }

            let current_key = partition
                .as_ref()
                .map(|fields| {
                    fields
                        .iter()
                        .map(|field| {
                            row.data
                                .get(&field.join("."))
                                .unwrap_or(&"".to_string())
                                .clone()
                        })
                        .collect::<Vec<_>>()
                        .join("|")
                })
                .unwrap_or("all".to_string());

            if let Some(group) = partitioned.get(&current_key) {
                let mut group_vec: Vec<(usize, Row)> = group.clone();
                group_vec.sort_by(|(i_a, a), (i_b, b)| {
                    order
                        .iter()
                        .fold(std::cmp::Ordering::Equal, |acc, (ord_expr, dir)| {
                            let a_val = evaluate_expr_with_db(
                                ord_expr,
                                a,
                                rows,
                                *i_a,
                                db,
                                base_collection,
                                schema,
                                functions,
                            )
                            .unwrap_or_default();
                            let b_val = evaluate_expr_with_db(
                                ord_expr,
                                b,
                                rows,
                                *i_b,
                                db,
                                base_collection,
                                schema,
                                functions,
                            )
                            .unwrap_or_default();
                            let cmp = a_val.cmp(&b_val);
                            match dir {
                                Direction::Asc => acc.then(cmp),
                                Direction::Desc => acc.then(cmp.reverse()),
                            }
                        })
                });

                let pos = group_vec
                    .iter()
                    .position(|(i, _)| *i == row_idx)
                    .unwrap_or(0);
                let lead_idx = pos + 1;
                if lead_idx < group_vec.len() {
                    let lead_row = &group_vec[lead_idx].1;
                    evaluate_expr_with_db(
                        expr,
                        lead_row,
                        rows,
                        group_vec[lead_idx].0,
                        db,
                        base_collection,
                        schema,
                        functions,
                    )
                } else {
                    Ok("null".to_string())
                }
            } else {
                Ok("null".to_string())
            }
        }
    }
}

fn evaluate_group_expr(
    expr: &Expr,
    rows: &[Row],
    db: &Db,
    base_schema_name: &str,
    schema: Option<&HashMap<String, FieldType>>,
    functions: &HashMap<String, Function>,
) -> Result<String, Box<dyn Error>> {
    match expr {
        Expr::Literal(lit) => Ok(match lit {
            Literal::Number(n) => n.to_string(),
            Literal::String(s) => s.clone(),
            Literal::Bool(b) => b.to_string(),
            Literal::Null => "null".to_string(),
            Literal::Object(obj) => {
                let mut map = HashMap::new();
                for (k, v) in obj {
                    map.insert(
                        k.clone(),
                        evaluate_group_expr(v, rows, db, base_schema_name, schema, functions)?,
                    );
                }
                serde_json::to_string(&map)?
            }
            Literal::GeoPoint(geo) => serde_json::to_string(geo)?,
        }),
        Expr::FieldPath(path) => Ok(rows
            .first()
            .and_then(|r| r.data.get(&path.join(".")))
            .cloned()
            .unwrap_or_default()),
        Expr::Arithmetic { left, op, right } => {
            let left_val =
                evaluate_group_expr(left, rows, db, base_schema_name, schema, functions)?
                    .parse::<f64>()?;
            let right_val =
                evaluate_group_expr(right, rows, db, base_schema_name, schema, functions)?
                    .parse::<f64>()?;
            let result = match op.as_str() {
                "+" => left_val + right_val,
                "-" => left_val - right_val,
                "*" => left_val * right_val,
                "/" => left_val / right_val,
                _ => return Err(format!("Unsupported operator '{}'", op).into()),
            };
            Ok(result.to_string())
        }
        Expr::Function { name, args } => match name.as_str() {
            "count" => {
                if args.len() != 1 {
                    return Err("count expects exactly one argument".into());
                }
                let field = match &args[0] {
                    Expr::FieldPath(path) => path.join("."),
                    _ => return Err("count argument must be a field path".into()),
                };
                let count = rows
                    .iter()
                    .filter(|row| row.data.contains_key(&field))
                    .count();
                Ok(count.to_string())
            }
            "avg" => {
                let values: Vec<f64> = args
                    .iter()
                    .flat_map(|arg| {
                        rows.iter().filter_map(|row| {
                            evaluate_expr_with_db(arg, row, db, base_schema_name, schema, functions)
                                .ok()
                                .and_then(|v| v.parse().ok())
                        })
                    })
                    .collect();
                let avg = if values.is_empty() {
                    0.0
                } else {
                    values.iter().sum::<f64>() / values.len() as f64
                };
                Ok(avg.to_string())
            }
            _ => Err(format!("Unknown function '{}'", name).into()),
        },
        Expr::Case {
            conditions,
            default,
        } => {
            for (cond, expr) in conditions {
                if evaluate_condition(
                    cond,
                    rows.first().unwrap_or(&Row {
                        data: HashMap::new(),
                    }),
                    db,
                    base_schema_name,
                    schema,
                    functions,
                )? {
                    return evaluate_group_expr(
                        expr,
                        rows,
                        db,
                        base_schema_name,
                        schema,
                        functions,
                    );
                }
            }
            default.as_ref().map_or(Ok("null".to_string()), |def| {
                evaluate_group_expr(def, rows, db, base_schema_name, schema, functions)
            })
        }
        Expr::Lag {
            expr,
            partition: _,
            order: _,
        }
        | Expr::Lead {
            expr,
            partition: _,
            order: _,
        } => {
            let new_hash_map_data = Row {
                data: HashMap::new(),
            };
            let first_row = rows.first().unwrap_or(&new_hash_map_data);
            evaluate_expr_with_db(expr, first_row, db, base_schema_name, schema, functions)
        }
    }
}

fn evaluate_condition(
    condition: &Condition,
    row: &Row,
    db: &Db,
    base_collection: &str,
    schema: Option<&HashMap<String, FieldType>>,
    functions: &HashMap<String, Function>,
) -> Result<bool, Box<dyn Error>> {
    let left = evaluate_expr_with_db(&condition.left, row, db, base_collection, schema, functions)?;
    let right = evaluate_expr_with_db(
        &condition.right,
        row,
        db,
        base_collection,
        schema,
        functions,
    )?;
    match condition.op.as_str() {
        "==" => Ok(left == right),
        "!=" => Ok(left != right),
        ">" => Ok(left.parse::<f64>()? > right.parse::<f64>()?),
        "<" => Ok(left.parse::<f64>()? < right.parse::<f64>()?),
        ">=" => Ok(left.parse::<f64>()? >= right.parse::<f64>()?),
        "<=" => Ok(left.parse::<f64>()? <= right.parse::<f64>()?),
        _ => Err(format!("Unknown operator: {}", condition.op).into()),
    }
}

fn haversine_distance(p1: &GeoPointLiteral, p2: &GeoPointLiteral) -> f64 {
    let r = 6371.0; // Earth's radius in kilometers
    let d_lat = (p2.lat - p1.lat).to_radians();
    let d_lon = (p2.lon - p1.lon).to_radians();
    let a = (d_lat / 2.0).sin().powi(2)
        + p1.lat.to_radians().cos() * p2.lat.to_radians().cos() * (d_lon / 2.0).sin().powi(2);
    2.0 * r * a.sqrt().atan2((1.0 - a).sqrt())
}
