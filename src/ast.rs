use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug)]
pub struct Program {
    pub schemas: Vec<Schema>,
    pub queries: Vec<Query>,
    pub functions: Vec<Function>,
    pub result: Option<HashMap<String, String>>,
}

#[derive(Debug)]
pub struct Schema {
    pub name: String,
    pub fields: HashMap<String, FieldType>,
}

#[derive(Debug)]
pub struct Query {
    pub name: String,
    pub source: QuerySource,
    pub operators: Vec<Operator>,
}

#[derive(Debug)]
pub enum QuerySource {
    Collection(String),
    Query(String),
}

#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub params: Vec<(String, Type)>,
    pub return_type: Type,
    pub body: FunctionBody,
}

#[derive(Debug, Clone)]
pub enum FunctionBody {
    Query(Vec<Operator>),
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub enum Type {
    Number,
    Bool,
    String,
    GeoPoint { grid: Option<GridConfig> },
    Collection { fields: HashMap<String, Type> },
    Ref(String),
    Struct { fields: HashMap<String, Type> },
    Enum(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GridConfig {
    pub resolution: String,
}

#[derive(Debug, Clone)]
pub enum Operator {
    Filter {
        condition: Condition,
    },
    Select {
        fields: Vec<FieldAlias>,
    },
    BatchInsert {
        values: Vec<HashMap<String, Expr>>,
    },
    GeoWithin {
        field: Vec<String>,
        bounds: GeoBounds,
    },
    Lead {
        name: String,
        field: Vec<String>,
        offset: i32,
        partition: Option<Vec<Vec<String>>>,
        order: Vec<(Expr, Direction)>,
    },
    Group {
        by: Expr,
        alias: String,
    }, // Updated
    Sum {
        field: Vec<String>,
    }, // Added
    Count {
        field: Vec<String>,
    }, // Simplified
    // Keep advanced operators for future use
    Sort {
        fields: Vec<(Expr, Direction)>,
    },
    Limit {
        count: usize,
    },
    Skip {
        count: usize,
    },
    Rank {
        name: String,
        partition: Option<Vec<Vec<String>>>,
        order: Vec<(Expr, Direction)>,
    },
    Insert {
        values: HashMap<String, Expr>,
    },
    Accumulate {
        name: String,
        field: Expr,
        partition: Option<Vec<Vec<String>>>,
    },
    TimeWindow {
        name: String,
        field: Expr,
        window: String,
        date_field: String,
        partition: Option<Vec<Vec<String>>>,
    },
    Avg {
        name: String,
        field: Vec<String>,
    },
    Max {
        name: String,
        field: Vec<String>,
    },
    Min {
        name: String,
        field: Vec<String>,
    },
    GeoDistance {
        name: String,
        field: Vec<String>,
        point: GeoPointLiteral,
        max_distance: f64,
    },
    Lag {
        name: String,
        field: Vec<String>,
        offset: i32,
        partition: Option<Vec<Vec<String>>>,
        order: Vec<(Expr, Direction)>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FieldType {
    String,
    Int8,
    Float, // Consider renaming to Number for consistency
    Bool,
    DateTime,
    GeoPoint { grid: Option<GridConfig> },
    Ref(String),
    Enum(Vec<String>),
    Struct(HashMap<String, FieldType>),
}

#[derive(Debug, Clone)]
pub struct FieldAlias {
    pub name: String,
    pub expr: Expr,
}

#[derive(Debug, Clone)]
pub enum Expr {
    FieldPath(Vec<String>),
    Function {
        name: String,
        args: Vec<Expr>,
    },
    Literal(Literal),
    Arithmetic {
        left: Box<Expr>,
        op: String,
        right: Box<Expr>,
    },
    Case {
        conditions: Vec<(Condition, Expr)>,
        default: Option<Box<Expr>>,
    },
    Lag {
        expr: Box<Expr>,
        partition: Option<Vec<Vec<String>>>,
        order: Option<Vec<(Expr, Direction)>>,
    },
    Lead {
        expr: Box<Expr>,
        partition: Option<Vec<Vec<String>>>,
        order: Option<Vec<(Expr, Direction)>>,
    },
}

#[derive(Debug, Clone)]
pub enum Literal {
    Number(f64),
    String(String),
    Bool(bool),
    Null,
    Object(HashMap<String, Expr>),
    GeoPoint(GeoPointLiteral),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoPointLiteral {
    pub lat: f64,
    pub lon: f64,
}

#[derive(Debug, Clone)]
pub struct GeoBounds {
    pub min_lat: f64,
    pub max_lat: f64,
    pub min_lon: f64,
    pub max_lon: f64,
}

#[derive(Debug, Clone)]
pub struct Condition {
    pub left: Expr,
    pub op: String,
    pub right: Expr,
}

#[derive(Debug, Clone)]
pub enum Direction {
    Asc,
    Desc,
}
