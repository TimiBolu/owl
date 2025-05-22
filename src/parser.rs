use crate::ast::*;
use crate::tokenizer::{Token, TokenKind};
use std::collections::HashMap;
use std::error::Error;

pub struct Parser {
    tokens: Vec<Token>,
    index: usize,
    schemas: HashMap<String, HashMap<String, FieldType>>,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser {
            tokens,
            index: 0,
            schemas: HashMap::new(),
        }
    }

    fn peek(&self) -> &Token {
        self.tokens
            .get(self.index)
            .unwrap_or(&self.tokens[self.tokens.len() - 1])
    }

    fn advance(&mut self) {
        if self.index < self.tokens.len() {
            self.index += 1;
        }
    }

    fn expect(&mut self, kind: TokenKind) -> Result<&Token, String> {
        let current_index = self.index;
        if self.peek().kind == kind {
            self.advance();
            Ok(&self.tokens[current_index])
        } else {
            Err(format!("Expected {:?}, found {:?}", kind, self.peek()))
        }
    }

    fn expect_value(&mut self, value: &str) -> Result<&Token, Box<dyn Error>> {
        let current_index = self.index;
        if self.peek().value == value {
            self.advance();
            Ok(&self.tokens[current_index])
        } else {
            Err(format!("Expected '{}', found '{}'", value, self.peek().value).into())
        }
    }

    pub fn parse_program(&mut self) -> Result<Program, Box<dyn Error>> {
        let mut schemas = Vec::new();
        let mut queries = Vec::new();
        let mut functions = Vec::new();
        let mut result = None;

        while self.peek().kind != TokenKind::EOF {
            println!("Current token: {:?}", self.peek());
            let next = self.tokens.get(self.index + 1).ok_or("Unexpected EOF")?;
            println!("Next token: {:?}", next);
            if self.peek().kind == TokenKind::Ident {
                if next.kind == TokenKind::LBrace {
                    let schema = self.parse_schema()?;
                    self.schemas
                        .insert(schema.name.clone(), schema.fields.clone());
                    schemas.push(schema);
                } else if next.kind == TokenKind::Operator && next.value == "=" {
                    queries.push(self.parse_query()?);
                } else if self.peek().value == "fn" {
                    functions.push(self.parse_function()?);
                } else if self.peek().value == "type" {
                    self.advance();
                    let schema = self.parse_schema()?;
                    schemas.push(schema);
                } else if self.peek().value == "enum" {
                    self.advance();
                    let schema = self.parse_schema()?;
                    schemas.push(schema);
                } else if self.peek().value == "sys" && next.value == "::" {
                    result = Some(self.parse_sys_result()?);
                    break;
                } else {
                    return Err(format!(
                        "Unexpected token after '{}': {:?}",
                        self.peek().value,
                        next
                    )
                    .into());
                }
            } else {
                return Err(format!("Unexpected token: {:?}", self.peek()).into());
            }
        }

        self.expect(TokenKind::EOF)?;
        for schema in &schemas {
            for (field_name, field_type) in &schema.fields {
                if let FieldType::Ref(ref_name) = field_type {
                    if !self.schemas.contains_key(ref_name) {
                        return Err(format!(
                            "Field '{}' in schema '{}' references undefined schema '{}'",
                            field_name, schema.name, ref_name
                        )
                        .into());
                    }
                }
            }
        }

        Ok(Program {
            schemas,
            queries,
            functions,
            result,
        })
    }

    fn parse_schema(&mut self) -> Result<Schema, Box<dyn Error>> {
        let name = self.expect(TokenKind::Ident)?.value.clone();
        self.expect(TokenKind::LBrace)?;
        let fields = if name == "enum" {
            let mut variants = Vec::new();
            while self.peek().kind != TokenKind::RBrace {
                variants.push(self.expect(TokenKind::Ident)?.value.clone());
                if self.peek().kind == TokenKind::Comma {
                    self.advance();
                }
            }
            HashMap::from([("variants".to_string(), FieldType::Enum(variants))])
        } else {
            let mut fields = HashMap::new();
            while self.peek().kind != TokenKind::RBrace {
                let field_name = self.expect(TokenKind::Ident)?.value.clone();
                println!("Field: {}", field_name);
                let field_type = self.parse_field_type()?;
                fields.insert(field_name, field_type);
            }
            fields
        };
        self.expect(TokenKind::RBrace)?;
        Ok(Schema { name, fields })
    }

    fn parse_field_type(&mut self) -> Result<FieldType, Box<dyn Error>> {
        let type_name = self.expect(TokenKind::Ident)?.value.clone();
        println!("Parsing field type: {}", type_name);
        match type_name.as_str() {
            "String" => Ok(FieldType::String),
            "Int8" => Ok(FieldType::Int8),
            "Float" => Ok(FieldType::Float),
            "Bool" => Ok(FieldType::Bool),
            "DateTime" => Ok(FieldType::DateTime),
            "Number" => Ok(FieldType::Float), // Map to Float for now
            "GeoPoint" => {
                if self.peek().kind == TokenKind::LParen {
                    self.advance();
                    self.expect(TokenKind::Ident)?; // "grid"
                    self.expect(TokenKind::Colon)?;
                    self.expect(TokenKind::LBrace)?;
                    self.expect(TokenKind::Ident)?; // "resolution"
                    self.expect(TokenKind::Colon)?;
                    let resolution = self.expect(TokenKind::String)?.value.clone();
                    self.expect(TokenKind::RBrace)?;
                    self.expect(TokenKind::RParen)?;
                    Ok(FieldType::GeoPoint {
                        grid: Some(GridConfig { resolution }),
                    })
                } else {
                    Ok(FieldType::GeoPoint { grid: None })
                }
            }
            "ref" => {
                self.expect(TokenKind::LParen)?;
                let ref_collection = self.expect(TokenKind::Ident)?.value.clone();
                self.expect(TokenKind::RParen)?;
                Ok(FieldType::Ref(ref_collection))
            }
            "enum" => {
                self.expect(TokenKind::LBrace)?;
                let mut variants = Vec::new();
                while self.peek().kind != TokenKind::RBrace {
                    variants.push(self.expect(TokenKind::Ident)?.value.clone());
                    if self.peek().kind == TokenKind::Comma {
                        self.advance();
                    }
                }
                self.expect(TokenKind::RBrace)?;
                Ok(FieldType::Enum(variants))
            }
            _ => Ok(FieldType::Ref(type_name)),
        }
    }

    fn parse_function(&mut self) -> Result<Function, Box<dyn Error>> {
        let fn_token = self.expect(TokenKind::Keyword)?;
        if fn_token.value != "fn" {
            return Err("Expected 'fn' keyword".into());
        }
        let name = self.expect(TokenKind::Ident)?.value.clone();
        self.expect(TokenKind::LParen)?;
        let mut params = Vec::new();
        while self.peek().kind != TokenKind::RParen {
            let param_name = self.expect(TokenKind::Ident)?.value.clone();
            let param_type = self.parse_type()?;
            params.push((param_name, param_type));
            if self.peek().kind == TokenKind::Comma {
                self.advance();
            }
        }
        self.expect(TokenKind::RParen)?;
        self.expect(TokenKind::Operator)?; // "->"
        let return_type = self.parse_type()?;
        self.expect(TokenKind::LBrace)?;
        let body = if matches!(return_type, Type::Bool) {
            let expr = self.parse_expression()?;
            FunctionBody::Expr(expr)
        } else {
            let mut operators = Vec::new();
            while self.peek().kind != TokenKind::RBrace {
                if self.peek().kind == TokenKind::Operator && self.peek().value == "::" {
                    self.advance();
                }
                operators.push(self.parse_operator()?);
            }
            FunctionBody::Query(operators)
        };
        self.expect(TokenKind::RBrace)?;
        Ok(Function {
            name,
            params,
            return_type,
            body,
        })
    }

    fn parse_type(&mut self) -> Result<Type, Box<dyn Error>> {
        let type_name = self.expect(TokenKind::Ident)?.value.clone();
        match type_name.as_str() {
            "Number" => Ok(Type::Number),
            "Bool" => Ok(Type::Bool),
            "String" => Ok(Type::String),
            "GeoPoint" => {
                if self.peek().kind == TokenKind::LParen {
                    self.advance();
                    self.expect(TokenKind::Ident)?; // "grid"
                    self.expect(TokenKind::Colon)?;
                    self.expect(TokenKind::LBrace)?;
                    self.expect(TokenKind::Ident)?; // "resolution"
                    self.expect(TokenKind::Colon)?;
                    let resolution = self.expect(TokenKind::String)?.value.clone();
                    self.expect(TokenKind::RBrace)?;
                    self.expect(TokenKind::RParen)?;
                    Ok(Type::GeoPoint {
                        grid: Some(GridConfig { resolution }),
                    })
                } else {
                    Ok(Type::GeoPoint { grid: None })
                }
            }
            "Collection" => {
                self.expect(TokenKind::LBrace)?;
                let mut fields = HashMap::new();
                while self.peek().kind != TokenKind::RBrace {
                    let field_name = self.expect(TokenKind::Ident)?.value.clone();
                    self.expect(TokenKind::Colon)?;
                    let field_type = self.parse_field_type()?;
                    // Map FieldType to Type
                    let mapped_type = match field_type {
                        FieldType::String => Type::String,
                        FieldType::Int8 | FieldType::Float => Type::Number, // Map Int8/Float to Number
                        FieldType::Bool => Type::Bool,
                        FieldType::DateTime => Type::String, // Approximate DateTime as String
                        FieldType::GeoPoint { grid } => Type::GeoPoint { grid },
                        FieldType::Ref(ref_name) => Type::Ref(ref_name),
                        FieldType::Enum(variants) => Type::Enum(variants),
                        FieldType::Struct(fields) => {
                            let mut mapped_fields = HashMap::new();
                            for (name, ft) in fields {
                                let mt = match ft {
                                    FieldType::String => Type::String,
                                    FieldType::Int8 | FieldType::Float => Type::Number,
                                    FieldType::Bool => Type::Bool,
                                    FieldType::DateTime => Type::String,
                                    FieldType::GeoPoint { grid } => Type::GeoPoint { grid },
                                    FieldType::Ref(ref_name) => Type::Ref(ref_name),
                                    FieldType::Enum(variants) => Type::Enum(variants),
                                    FieldType::Struct(_) => {
                                        return Err(
                                            "Nested structs not supported in Type::Collection"
                                                .into(),
                                        )
                                    }
                                };
                                mapped_fields.insert(name, mt);
                            }
                            Type::Struct {
                                fields: mapped_fields,
                            }
                        }
                    };
                    fields.insert(field_name, mapped_type);
                }
                self.expect(TokenKind::RBrace)?;
                Ok(Type::Collection { fields })
            }
            _ => Ok(Type::Ref(type_name)),
        }
    }

    fn parse_query(&mut self) -> Result<Query, Box<dyn Error>> {
        let name = self.expect(TokenKind::Ident)?.value.clone();
        println!("Parsing query: {}", name);
        self.expect(TokenKind::Operator)?; // "="
        let source_ident = self.expect(TokenKind::Ident)?.value.clone();
        let mut operators = Vec::new();
        if self.peek().kind == TokenKind::Operator && self.peek().value == "::" {
            self.advance();
            println!(
                "Parsing initial operator after '::', next token: {:?}",
                self.peek()
            );
            operators.push(self.parse_operator()?);
            while self.peek().kind == TokenKind::Operator && self.peek().value == "::" {
                self.advance();
                println!("Parsing operator after '::', next token: {:?}", self.peek());
                operators.push(self.parse_operator()?);
            }
        }
        println!("Finished query {}, next token: {:?}", name, self.peek());
        Ok(Query {
            name,
            source: QuerySource::Collection(source_ident),
            operators,
        })
    }

    fn parse_operator(&mut self) -> Result<Operator, Box<dyn Error>> {
        let op_name = self.expect(TokenKind::Ident)?.value.clone();
        println!("Parsing operator: {}", op_name);
        match op_name.as_str() {
            "filter" => {
                self.expect(TokenKind::LBrace)?;
                let condition = self.parse_condition()?;
                self.expect(TokenKind::RBrace)?;
                Ok(Operator::Filter { condition })
            }
            "select" => {
                self.expect(TokenKind::LBrace)?;
                let fields = self.parse_select()?;
                self.expect(TokenKind::RBrace)?;
                Ok(Operator::Select { fields })
            }
            "batch_insert" => {
                println!("Starting batch_insert, next token: {:?}", self.peek());
                self.expect(TokenKind::LBracket)?;
                let mut batch_values = Vec::new();
                while self.peek().kind != TokenKind::RBracket {
                    self.expect(TokenKind::LBrace)?;
                    let mut values = HashMap::new();
                    while self.peek().kind != TokenKind::RBrace {
                        let key = self.expect(TokenKind::Ident)?.value.clone();
                        println!("Key: {}, next token: {:?}", key, self.peek());
                        self.expect(TokenKind::Colon)?;
                        let expr = if self.peek().kind == TokenKind::LBrace {
                            self.advance();
                            let mut nested = HashMap::new();
                            while self.peek().kind != TokenKind::RBrace {
                                let nested_key = self.expect(TokenKind::Ident)?.value.clone();
                                self.expect(TokenKind::Colon)?;
                                let nested_value = self.parse_expression()?;
                                nested.insert(nested_key, nested_value);
                                if self.peek().kind == TokenKind::Comma {
                                    self.advance();
                                }
                            }
                            self.expect(TokenKind::RBrace)?;
                            Expr::Literal(Literal::Object(nested))
                        } else {
                            self.parse_expression()?
                        };
                        values.insert(key, expr);
                        if self.peek().kind == TokenKind::Comma {
                            self.advance();
                        }
                    }
                    self.expect(TokenKind::RBrace)?;
                    batch_values.push(values);
                    if self.peek().kind == TokenKind::Comma {
                        self.advance();
                    }
                }
                self.expect(TokenKind::RBracket)?;
                println!("Finished batch_insert, next token: {:?}", self.peek());
                Ok(Operator::BatchInsert {
                    values: batch_values,
                })
            }
            "geo_within" => {
                self.expect(TokenKind::LBrace)?;
                self.expect(TokenKind::Ident)?; // "field"
                self.expect(TokenKind::Colon)?;
                let field = self.parse_field_path()?;
                self.expect(TokenKind::Comma)?;
                self.expect(TokenKind::Ident)?; // "bounds"
                self.expect(TokenKind::Colon)?;
                self.expect(TokenKind::LBrace)?;
                self.expect(TokenKind::Ident)?; // "min_lat"
                self.expect(TokenKind::Colon)?;
                let min_lat = self.parse_expression()?.as_number()?;
                self.expect(TokenKind::Comma)?;
                self.expect(TokenKind::Ident)?; // "max_lat"
                self.expect(TokenKind::Colon)?;
                let max_lat = self.parse_expression()?.as_number()?;
                self.expect(TokenKind::Comma)?;
                self.expect(TokenKind::Ident)?; // "min_lon"
                self.expect(TokenKind::Colon)?;
                let min_lon = self.parse_expression()?.as_number()?;
                self.expect(TokenKind::Comma)?;
                self.expect(TokenKind::Ident)?; // "max_lon"
                self.expect(TokenKind::Colon)?;
                let max_lon = self.parse_expression()?.as_number()?;
                self.expect(TokenKind::RBrace)?;
                self.expect(TokenKind::RBrace)?;
                Ok(Operator::GeoWithin {
                    field,
                    bounds: GeoBounds {
                        min_lat,
                        max_lat,
                        min_lon,
                        max_lon,
                    },
                })
            }
            "lead" => {
                self.expect(TokenKind::LBrace)?;
                self.expect(TokenKind::Ident)?; // "name"
                self.expect(TokenKind::Colon)?;
                let name = self.expect(TokenKind::Ident)?.value.clone();
                self.expect(TokenKind::Comma)?;
                self.expect(TokenKind::Ident)?; // "field"
                self.expect(TokenKind::Colon)?;
                let field = self.parse_field_path()?;
                self.expect(TokenKind::Comma)?;
                self.expect(TokenKind::Ident)?; // "offset"
                self.expect(TokenKind::Colon)?;
                let offset = self.expect(TokenKind::Number)?.value.parse::<i32>()?;
                let mut partition = None;
                let mut order = Vec::new();
                if self.peek().kind == TokenKind::Comma {
                    self.advance();
                    if self.peek().value == "partition" {
                        self.advance();
                        self.expect(TokenKind::Keyword)?; // "by"
                        let mut parts = Vec::new();
                        while self.peek().kind != TokenKind::Keyword
                            && self.peek().kind != TokenKind::RBrace
                        {
                            parts.push(self.parse_field_path()?);
                            if self.peek().kind == TokenKind::Comma {
                                self.advance();
                            }
                        }
                        partition = Some(parts);
                    }
                    if self.peek().value == "order" {
                        self.advance();
                        self.expect(TokenKind::Keyword)?; // "by"
                        while self.peek().kind != TokenKind::RBrace {
                            let expr = self.parse_expression()?;
                            let direction = if self.peek().kind == TokenKind::Keyword {
                                let dir = self.peek().value.clone();
                                self.advance();
                                match dir.as_str() {
                                    "asc" => Direction::Asc,
                                    "desc" => Direction::Desc,
                                    _ => {
                                        return Err(format!(
                                            "Expected 'asc' or 'desc', found '{}'",
                                            dir
                                        )
                                        .into())
                                    }
                                }
                            } else {
                                Direction::Asc
                            };
                            order.push((expr, direction));
                            if self.peek().kind == TokenKind::Comma {
                                self.advance();
                            }
                        }
                    }
                }
                self.expect(TokenKind::RBrace)?;
                Ok(Operator::Lead {
                    name,
                    field,
                    offset,
                    partition,
                    order,
                })
            }
            "group" => {
                self.expect(TokenKind::LBrace)?;
                self.expect_value("by")?;
                let by = self.parse_expression()?;
                self.expect(TokenKind::Keyword)?; // "as"
                if self.peek().value != "as" {
                    return Err("Expected 'as' keyword".into());
                }
                let alias = self.expect(TokenKind::Ident)?.value.clone();
                self.expect(TokenKind::RBrace)?;
                Ok(Operator::Group { by, alias })
            }
            "sum" => {
                self.expect(TokenKind::LBrace)?;
                let field = self.parse_field_path()?;
                self.expect(TokenKind::RBrace)?;
                Ok(Operator::Sum { field })
            }
            "count" => {
                self.expect(TokenKind::LBrace)?;
                let field = self.parse_field_path()?;
                self.expect(TokenKind::RBrace)?;
                Ok(Operator::Count { field })
            }
            "sort" => {
                self.expect(TokenKind::LBrace)?;
                let mut fields = Vec::new();
                while self.peek().kind != TokenKind::RBrace {
                    let expr = self.parse_expression()?;
                    let direction = if self.peek().kind == TokenKind::Keyword {
                        let dir = self.peek().value.clone();
                        self.advance();
                        match dir.as_str() {
                            "asc" => Direction::Asc,
                            "desc" => Direction::Desc,
                            _ => {
                                return Err(
                                    format!("Expected 'asc' or 'desc', found '{}'", dir).into()
                                )
                            }
                        }
                    } else {
                        Direction::Asc
                    };
                    fields.push((expr, direction));
                    if self.peek().kind == TokenKind::Comma {
                        self.advance();
                    }
                }
                self.expect(TokenKind::RBrace)?;
                Ok(Operator::Sort { fields })
            }
            "limit" => {
                self.expect(TokenKind::LBrace)?;
                let count = self.expect(TokenKind::Number)?.value.parse::<usize>()?;
                self.expect(TokenKind::RBrace)?;
                Ok(Operator::Limit { count })
            }
            "skip" => {
                self.expect(TokenKind::LBrace)?;
                let count = self.expect(TokenKind::Number)?.value.parse::<usize>()?;
                self.expect(TokenKind::RBrace)?;
                Ok(Operator::Skip { count })
            }
            "rank" => {
                self.expect(TokenKind::LBrace)?;
                self.expect_value("name")?;
                self.expect(TokenKind::Colon)?;
                let name = self.expect(TokenKind::Ident)?.value.clone();
                let mut partition = None;
                let mut order = Vec::new();
                if self.peek().kind == TokenKind::Comma {
                    self.advance();
                    if self.peek().value == "partition" {
                        self.advance();
                        self.expect_value("by")?;
                        let mut parts = Vec::new();
                        while self.peek().kind != TokenKind::Keyword
                            && self.peek().kind != TokenKind::RBrace
                        {
                            parts.push(self.parse_field_path()?);
                            if self.peek().kind == TokenKind::Comma {
                                self.advance();
                            }
                        }
                        partition = Some(parts);
                    }
                    if self.peek().value == "order" {
                        self.advance();
                        self.expect_value("by")?;
                        while self.peek().kind != TokenKind::RBrace {
                            let expr = self.parse_expression()?;
                            let direction = if self.peek().kind == TokenKind::Keyword {
                                let dir = self.peek().value.clone();
                                self.advance();
                                match dir.as_str() {
                                    "asc" => Direction::Asc,
                                    "desc" => Direction::Desc,
                                    _ => {
                                        return Err(format!(
                                            "Expected 'asc' or 'desc', found '{}'",
                                            dir
                                        )
                                        .into())
                                    }
                                }
                            } else {
                                Direction::Asc
                            };
                            order.push((expr, direction));
                            if self.peek().kind == TokenKind::Comma {
                                self.advance();
                            }
                        }
                    }
                }
                self.expect(TokenKind::RBrace)?;
                Ok(Operator::Rank {
                    name,
                    partition,
                    order,
                })
            }
            "insert" => {
                self.expect(TokenKind::LBrace)?;
                let mut values = HashMap::new();
                while self.peek().kind != TokenKind::RBrace {
                    let key = self.expect(TokenKind::Ident)?.value.clone();
                    self.expect(TokenKind::Colon)?;
                    let expr = self.parse_expression()?;
                    values.insert(key, expr);
                    if self.peek().kind == TokenKind::Comma {
                        self.advance();
                    }
                }
                self.expect(TokenKind::RBrace)?;
                Ok(Operator::Insert { values })
            }
            "accumulate" => {
                self.expect(TokenKind::LBrace)?;
                self.expect_value("name")?;
                self.expect(TokenKind::Colon)?;
                let name = self.expect(TokenKind::Ident)?.value.clone();
                self.expect(TokenKind::Comma)?;
                self.expect_value("field")?;
                self.expect(TokenKind::Colon)?;
                let field = self.parse_expression()?;
                let mut partition = None;
                if self.peek().kind == TokenKind::Comma {
                    self.advance();
                    if self.peek().value == "partition" {
                        self.advance();
                        self.expect_value("by")?;
                        let mut parts = Vec::new();
                        while self.peek().kind != TokenKind::RBrace {
                            parts.push(self.parse_field_path()?);
                            if self.peek().kind == TokenKind::Comma {
                                self.advance();
                            }
                        }
                        partition = Some(parts);
                    }
                }
                self.expect(TokenKind::RBrace)?;
                Ok(Operator::Accumulate {
                    name,
                    field,
                    partition,
                })
            }
            "time_window" => {
                self.expect(TokenKind::LBrace)?;
                self.expect_value("name")?;
                self.expect(TokenKind::Colon)?;
                let name = self.expect(TokenKind::Ident)?.value.clone();
                self.expect(TokenKind::Comma)?;
                self.expect_value("field")?;
                self.expect(TokenKind::Colon)?;
                let field = self.parse_expression()?;
                self.expect(TokenKind::Comma)?;
                self.expect_value("window")?;
                self.expect(TokenKind::Colon)?;
                let window = self.expect(TokenKind::String)?.value.clone();
                self.expect(TokenKind::Comma)?;
                self.expect_value("date_field")?;
                self.expect(TokenKind::Colon)?;
                let date_field = self.expect(TokenKind::Ident)?.value.clone();
                let mut partition = None;
                if self.peek().kind == TokenKind::Comma {
                    self.advance();
                    if self.peek().value == "partition" {
                        self.advance();
                        self.expect_value("by")?;
                        let mut parts = Vec::new();
                        while self.peek().kind != TokenKind::RBrace {
                            parts.push(self.parse_field_path()?);
                            if self.peek().kind == TokenKind::Comma {
                                self.advance();
                            }
                        }
                        partition = Some(parts);
                    }
                }
                self.expect(TokenKind::RBrace)?;
                Ok(Operator::TimeWindow {
                    name,
                    field,
                    window,
                    date_field,
                    partition,
                })
            }
            "avg" => {
                self.expect(TokenKind::LBrace)?;
                self.expect_value("name")?;
                self.expect(TokenKind::Colon)?;
                let name = self.expect(TokenKind::Ident)?.value.clone();
                self.expect(TokenKind::Comma)?;
                self.expect_value("field")?;
                self.expect(TokenKind::Colon)?;
                let field = self.parse_field_path()?;
                self.expect(TokenKind::RBrace)?;
                Ok(Operator::Avg { name, field })
            }
            "max" => {
                self.expect(TokenKind::LBrace)?;
                self.expect_value("name")?;
                self.expect(TokenKind::Colon)?;
                let name = self.expect(TokenKind::Ident)?.value.clone();
                self.expect(TokenKind::Comma)?;
                self.expect_value("field")?;
                self.expect(TokenKind::Colon)?;
                let field = self.parse_field_path()?;
                self.expect(TokenKind::RBrace)?;
                Ok(Operator::Max { name, field })
            }
            "min" => {
                self.expect(TokenKind::LBrace)?;
                self.expect_value("name")?;
                self.expect(TokenKind::Colon)?;
                let name = self.expect(TokenKind::Ident)?.value.clone();
                self.expect(TokenKind::Comma)?;
                self.expect_value("field")?;
                self.expect(TokenKind::Colon)?;
                let field = self.parse_field_path()?;
                self.expect(TokenKind::RBrace)?;
                Ok(Operator::Min { name, field })
            }
            "geo_distance" => {
                self.expect(TokenKind::LBrace)?;
                self.expect_value("name")?;
                self.expect(TokenKind::Colon)?;
                let name = self.expect(TokenKind::Ident)?.value.clone();
                self.expect(TokenKind::Comma)?;
                self.expect_value("field")?;
                self.expect(TokenKind::Colon)?;
                let field = self.parse_field_path()?;
                self.expect(TokenKind::Comma)?;
                self.expect_value("point")?;
                self.expect(TokenKind::Colon)?;
                self.expect(TokenKind::LBrace)?;
                self.expect_value("lat")?;
                self.expect(TokenKind::Colon)?;
                let lat = self.expect(TokenKind::Number)?.value.parse::<f64>()?;
                self.expect(TokenKind::Comma)?;
                self.expect_value("lon")?;
                self.expect(TokenKind::Colon)?;
                let lon = self.expect(TokenKind::Number)?.value.parse::<f64>()?;
                self.expect(TokenKind::RBrace)?;
                self.expect(TokenKind::Comma)?;
                self.expect_value("max_distance")?;
                self.expect(TokenKind::Colon)?;
                let max_distance = self.expect(TokenKind::Number)?.value.parse::<f64>()?;
                self.expect(TokenKind::RBrace)?;
                Ok(Operator::GeoDistance {
                    name,
                    field,
                    point: GeoPointLiteral { lat, lon },
                    max_distance,
                })
            }
            "lag" => {
                self.expect(TokenKind::LBrace)?;
                self.expect_value("name")?;
                self.expect(TokenKind::Colon)?;
                let name = self.expect(TokenKind::Ident)?.value.clone();
                self.expect(TokenKind::Comma)?;
                self.expect_value("field")?;
                self.expect(TokenKind::Colon)?;
                let field = self.parse_field_path()?;
                self.expect(TokenKind::Comma)?;
                self.expect_value("offset")?;
                self.expect(TokenKind::Colon)?;
                let offset = self.expect(TokenKind::Number)?.value.parse::<i32>()?;
                let mut partition = None;
                let mut order = Vec::new();
                if self.peek().kind == TokenKind::Comma {
                    self.advance();
                    if self.peek().value == "partition" {
                        self.advance();
                        self.expect_value("by")?;
                        let mut parts = Vec::new();
                        while self.peek().kind != TokenKind::Keyword
                            && self.peek().kind != TokenKind::RBrace
                        {
                            parts.push(self.parse_field_path()?);
                            if self.peek().kind == TokenKind::Comma {
                                self.advance();
                            }
                        }
                        partition = Some(parts);
                    }
                    if self.peek().value == "order" {
                        self.advance();
                        self.expect_value("by")?;
                        while self.peek().kind != TokenKind::RBrace {
                            let expr = self.parse_expression()?;
                            let direction = if self.peek().kind == TokenKind::Keyword {
                                let dir = self.peek().value.clone();
                                self.advance();
                                match dir.as_str() {
                                    "asc" => Direction::Asc,
                                    "desc" => Direction::Desc,
                                    _ => {
                                        return Err(format!(
                                            "Expected 'asc' or 'desc', found '{}'",
                                            dir
                                        )
                                        .into())
                                    }
                                }
                            } else {
                                Direction::Asc
                            };
                            order.push((expr, direction));
                            if self.peek().kind == TokenKind::Comma {
                                self.advance();
                            }
                        }
                    }
                }
                self.expect(TokenKind::RBrace)?;
                Ok(Operator::Lag {
                    name,
                    field,
                    offset,
                    partition,
                    order,
                })
            }
            _ => Err(format!("Unknown operator: {}", op_name).into()),
        }
    }

    fn parse_select(&mut self) -> Result<Vec<FieldAlias>, Box<dyn Error>> {
        let mut fields = Vec::new();
        self.expect(TokenKind::LBrace)?;
        while self.peek().kind != TokenKind::RBrace {
            let name = self.expect(TokenKind::Ident)?.value.clone();
            let expr = if self.peek().kind == TokenKind::Colon {
                self.advance();
                self.parse_expression()?
            } else {
                Expr::FieldPath(vec![name.clone()])
            };
            fields.push(FieldAlias { name, expr });
            if self.peek().kind == TokenKind::Comma {
                self.advance();
            }
        }
        self.expect(TokenKind::RBrace)?;
        Ok(fields)
    }

    fn parse_condition(&mut self) -> Result<Condition, Box<dyn Error>> {
        let left = if self.peek().kind == TokenKind::Dot {
            Expr::FieldPath(self.parse_field_path()?)
        } else {
            self.parse_expression()?
        };
        let op = self.expect(TokenKind::Operator)?.value.clone();
        let right = self.parse_expression()?;
        Ok(Condition { left, op, right })
    }

    fn parse_expression(&mut self) -> Result<Expr, Box<dyn Error>> {
        self.parse_expression_with_precedence(0)
    }

    fn parse_expression_with_precedence(
        &mut self,
        min_precedence: u8,
    ) -> Result<Expr, Box<dyn Error>> {
        let mut left = self.parse_primary()?;
        while self.peek().kind == TokenKind::Operator {
            let op = self.peek().value.clone();
            let precedence = match op.as_str() {
                "+" | "-" => 1,
                "*" | "/" => 2,
                "&&" => 3,
                ">=" | "<=" | ">" | "<" | "==" => 4,
                _ => break,
            };
            if precedence <= min_precedence {
                break;
            }
            self.advance();
            let right = self.parse_expression_with_precedence(precedence)?;
            left = Expr::Arithmetic {
                left: Box::new(left),
                op,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_primary(&mut self) -> Result<Expr, Box<dyn Error>> {
        match self.peek().kind {
            TokenKind::Dot => Ok(Expr::FieldPath(self.parse_field_path()?)),
            TokenKind::Ident => {
                let ident = self.peek().value.clone();
                self.advance();
                if self.peek().kind == TokenKind::LParen {
                    self.advance();
                    let mut args = Vec::new();
                    while self.peek().kind != TokenKind::RParen {
                        args.push(self.parse_expression()?);
                        if self.peek().kind == TokenKind::Comma {
                            self.advance();
                        }
                    }
                    self.expect(TokenKind::RParen)?;
                    Ok(Expr::Function { name: ident, args })
                } else if self.peek().kind == TokenKind::Dot {
                    let mut path = vec![ident];
                    while self.peek().kind == TokenKind::Dot {
                        self.advance();
                        path.push(self.expect(TokenKind::Ident)?.value.clone());
                    }
                    Ok(Expr::FieldPath(path))
                } else {
                    Ok(Expr::Literal(Literal::String(ident)))
                }
            }
            TokenKind::Number => {
                let num = self.peek().value.parse::<f64>()?;
                self.advance();
                Ok(Expr::Literal(Literal::Number(num)))
            }
            TokenKind::Operator if self.peek().value == "-" => {
                self.advance();
                if self.peek().kind == TokenKind::Number {
                    let num = self.peek().value.parse::<f64>()?;
                    self.advance();
                    Ok(Expr::Literal(Literal::Number(-num)))
                } else {
                    Err(format!("Expected number after '-', found {:?}", self.peek()).into())
                }
            }
            TokenKind::String => {
                let s = self.peek().value.clone();
                self.advance();
                Ok(Expr::Literal(Literal::String(s)))
            }
            TokenKind::True => {
                self.advance();
                Ok(Expr::Literal(Literal::Bool(true)))
            }
            TokenKind::False => {
                self.advance();
                Ok(Expr::Literal(Literal::Bool(false)))
            }
            TokenKind::Null => {
                self.advance();
                Ok(Expr::Literal(Literal::Null))
            }
            _ => Err(format!("Unexpected primary: {:?}", self.peek()).into()),
        }
    }

    fn parse_field_path(&mut self) -> Result<Vec<String>, Box<dyn Error>> {
        let mut path = Vec::new();
        if self.peek().kind == TokenKind::Dot {
            self.advance();
        }
        path.push(self.expect(TokenKind::Ident)?.value.clone());
        while self.peek().kind == TokenKind::Dot {
            self.advance();
            path.push(self.expect(TokenKind::Ident)?.value.clone());
        }
        Ok(path)
    }

    fn parse_sys_result(&mut self) -> Result<HashMap<String, String>, Box<dyn Error>> {
        self.expect_value("sys")?;
        self.expect(TokenKind::Operator)?; // "::"
        self.expect_value("result")?;
        self.expect(TokenKind::LBrace)?;
        let mut result_map = HashMap::new();
        while self.peek().kind != TokenKind::RBrace {
            let key = self.expect(TokenKind::Ident)?.value.clone();
            self.expect(TokenKind::Colon)?;
            let value = self.expect(TokenKind::Ident)?.value.clone();
            result_map.insert(key, value);
            if self.peek().kind == TokenKind::Comma {
                self.advance();
            }
        }
        self.expect(TokenKind::RBrace)?;
        Ok(result_map)
    }
}

trait AsNumber {
    fn as_number(self) -> Result<f64, Box<dyn Error>>;
}

impl AsNumber for Expr {
    fn as_number(self) -> Result<f64, Box<dyn Error>> {
        match self {
            Expr::Literal(Literal::Number(n)) => Ok(n),
            _ => Err("Expected number".into()),
        }
    }
}
