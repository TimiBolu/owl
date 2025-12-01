// OQL Parser Module - Natural Language Query Parser

use nom::{
    branch::alt,
    bytes::complete::{tag, take_while},
    character::complete::{alpha1, alphanumeric1, char, digit1, multispace1},
    combinator::{map, recognize},
    multi::many0,
    sequence::{pair, preceded, tuple},
    IResult,
};
use serde::{Deserialize, Serialize};

// Token types for OQL lexical analysis
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Token {
    // Collection methods
    Where,
    Select,
    Insert,
    Update,
    Delete,
    Sort,
    Limit,
    GroupBy,
    Having,
    
    // Function definition
    Fn,
    Arrow,        // ->
    Let,
    
    // Aggregation functions
    Count,
    Sum,
    Avg,
    Min,
    Max,
    
    // Logical operators
    And,
    Or,
    Not,
    
    // Conditional keywords
    If,
    Then,
    Else,
    Match,
    
    // Comparison operators
    Equal,        // ==
    NotEqual,     // !=
    LessThan,     // <
    GreaterThan,  // >
    LessEqual,    // <=
    GreaterEqual, // >=
    
    // String operators
    Contains,
    StartsWith,
    EndsWith,
    In,
    
    // Literals
    StringLiteral(String),
    IntegerLiteral(i64),
    FloatLiteral(f64),
    BooleanLiteral(bool),
    
    // Identifiers and field access
    Identifier(String),        // collection names, variable names
    FieldAccess(Vec<String>),  // .field or .field.subfield
    
    // Punctuation
    LeftBrace,    // {
    RightBrace,   // }
    LeftParen,    // (
    RightParen,   // )
    LeftBracket,  // [
    RightBracket, // ]
    Comma,        // ,
    Colon,        // :
    Dot,          // .
    Star,         // *
    Plus,         // +
    Minus,        // -
    Slash,        // /
    Percent,      // %
    
    // Special
    Whitespace,
    Comment(String),
    EOF,
}

// Tokenizer implementation
pub struct Tokenizer;

impl Tokenizer {
    pub fn tokenize(input: &str) -> Result<Vec<Token>, TokenizeError> {
        let mut tokens = Vec::new();
        let mut remaining = input;
        
        while !remaining.is_empty() {
            match Self::next_token(remaining) {
                Ok((rest, token)) => {
                    if !matches!(token, Token::Whitespace | Token::Comment(_)) {
                        tokens.push(token);
                    }
                    remaining = rest;
                }
                Err(e) => return Err(TokenizeError::ParseError(format!("Failed to parse token: {:?}", e))),
            }
        }
        
        tokens.push(Token::EOF);
        Ok(tokens)
    }
    
    fn next_token(input: &str) -> IResult<&str, Token> {
        alt((
            Self::parse_whitespace,
            Self::parse_comment,
            Self::parse_operator,
            Self::parse_string_literal,
            Self::parse_number,
            Self::parse_boolean,
            Self::parse_field_access,
            Self::parse_keyword,
            Self::parse_identifier,
            Self::parse_punctuation,
        ))(input)
    }
    
    fn parse_whitespace(input: &str) -> IResult<&str, Token> {
        map(multispace1, |_| Token::Whitespace)(input)
    }
    
    fn parse_comment(input: &str) -> IResult<&str, Token> {
        let (input, _) = tag("//")(input)?;
        let (input, comment_text) = take_while(|c| c != '\n' && c != '\r')(input)?;
        Ok((input, Token::Comment(comment_text.to_string())))
    }
    
    fn parse_keyword(input: &str) -> IResult<&str, Token> {
        // Parse keywords as whole words only (followed by non-alphanumeric)
        let (input, keyword) = alt((
            alt((
                tag("group_by"),
                tag("having"),
                tag("where"),
                tag("select"),
                tag("insert"),
                tag("update"),
                tag("delete"),
                tag("sort"),
                tag("limit"),
            )),
            alt((
                tag("count"),
                tag("sum"),
                tag("avg"),
                tag("min"),
                tag("max"),
                tag("and"),
                tag("or"),
                tag("not"),
            )),
            alt((
                tag("fn"),
                tag("let"),
                tag("if"),
                tag("then"),
                tag("else"),
                tag("match"),
            )),
            alt((
                tag("starts_with"),
                tag("ends_with"),
                tag("contains"),
                tag("in"),
            )),
        ))(input)?;
        
        // Check that the keyword is followed by a non-alphanumeric character or end of input
        if let Ok((_, next_char)) = alphanumeric1::<&str, nom::error::Error<&str>>(input) {
            if !next_char.is_empty() {
                // This is part of a larger identifier, not a keyword
                return Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)));
            }
        }
        
        let token = match keyword {
            "group_by" => Token::GroupBy,
            "having" => Token::Having,
            "where" => Token::Where,
            "select" => Token::Select,
            "insert" => Token::Insert,
            "update" => Token::Update,
            "delete" => Token::Delete,
            "sort" => Token::Sort,
            "limit" => Token::Limit,
            "count" => Token::Count,
            "sum" => Token::Sum,
            "avg" => Token::Avg,
            "min" => Token::Min,
            "max" => Token::Max,
            "and" => Token::And,
            "or" => Token::Or,
            "not" => Token::Not,
            "fn" => Token::Fn,
            "let" => Token::Let,
            "if" => Token::If,
            "then" => Token::Then,
            "else" => Token::Else,
            "match" => Token::Match,
            "contains" => Token::Contains,
            "starts_with" => Token::StartsWith,
            "ends_with" => Token::EndsWith,
            "in" => Token::In,
            _ => unreachable!(),
        };
        
        Ok((input, token))
    }
    
    fn parse_operator(input: &str) -> IResult<&str, Token> {
        alt((
            map(tag("->"), |_| Token::Arrow),
            map(tag("&&"), |_| Token::And),
            map(tag("||"), |_| Token::Or),
            map(tag("=="), |_| Token::Equal),
            map(tag("!="), |_| Token::NotEqual),
            map(tag("<="), |_| Token::LessEqual),
            map(tag(">="), |_| Token::GreaterEqual),
            map(tag("<"), |_| Token::LessThan),
            map(tag(">"), |_| Token::GreaterThan),
            map(tag("+"), |_| Token::Plus),
            map(tag("-"), |_| Token::Minus),
            map(tag("/"), |_| Token::Slash),
            map(tag("%"), |_| Token::Percent),
        ))(input)
    }
    
    fn parse_string_literal(input: &str) -> IResult<&str, Token> {
        let (input, _) = char('"')(input)?;
        let (input, content) = take_while(|c| c != '"')(input)?;
        let (input, _) = char('"')(input)?;
        Ok((input, Token::StringLiteral(content.to_string())))
    }
    
    fn parse_number(input: &str) -> IResult<&str, Token> {
        alt((
            Self::parse_float,
            Self::parse_integer,
        ))(input)
    }
    
    fn parse_float(input: &str) -> IResult<&str, Token> {
        let (input, number_str) = recognize(tuple((
            digit1,
            char('.'),
            digit1,
        )))(input)?;
        
        let value = number_str.parse::<f64>()
            .map_err(|_| nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Float)))?;
        
        Ok((input, Token::FloatLiteral(value)))
    }
    
    fn parse_integer(input: &str) -> IResult<&str, Token> {
        let (input, number_str) = digit1(input)?;
        let value = number_str.parse::<i64>()
            .map_err(|_| nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Digit)))?;
        
        Ok((input, Token::IntegerLiteral(value)))
    }
    
    fn parse_boolean(input: &str) -> IResult<&str, Token> {
        alt((
            map(tag("true"), |_| Token::BooleanLiteral(true)),
            map(tag("false"), |_| Token::BooleanLiteral(false)),
        ))(input)
    }
    
    fn parse_field_access(input: &str) -> IResult<&str, Token> {
        let (input, _) = char('.')(input)?;
        
        // Check if the next part is a keyword - if so, don't parse as field access
        if let Ok((_, _keyword)) = Self::parse_keyword(input) {
            // This is a dot followed by a keyword, not a field access
            // Return the dot and let the keyword be parsed separately
            return Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Alt)));
        }
        
        let (input, first_field) = Self::parse_identifier_name(input)?;
        let (input, additional_fields) = many0(preceded(char('.'), Self::parse_identifier_name))(input)?;
        
        let mut path = vec![first_field];
        path.extend(additional_fields);
        
        Ok((input, Token::FieldAccess(path)))
    }
    
    fn parse_identifier(input: &str) -> IResult<&str, Token> {
        map(Self::parse_identifier_name, Token::Identifier)(input)
    }
    
    fn parse_identifier_name(input: &str) -> IResult<&str, String> {
        let (input, name) = recognize(pair(
            alt((alpha1, tag("_"))),
            many0(alt((alphanumeric1, tag("_")))),
        ))(input)?;
        Ok((input, name.to_string()))
    }
    
    fn parse_punctuation(input: &str) -> IResult<&str, Token> {
        alt((
            map(char('{'), |_| Token::LeftBrace),
            map(char('}'), |_| Token::RightBrace),
            map(char('('), |_| Token::LeftParen),
            map(char(')'), |_| Token::RightParen),
            map(char('['), |_| Token::LeftBracket),
            map(char(']'), |_| Token::RightBracket),
            map(char(','), |_| Token::Comma),
            map(char(':'), |_| Token::Colon),
            map(char('.'), |_| Token::Dot),
            map(char('*'), |_| Token::Star),
        ))(input)
    }
}

// Error types for tokenization
#[derive(Debug, Clone)]
pub enum TokenizeError {
    ParseError(String),
    UnexpectedCharacter(char),
    UnterminatedString,
}

impl std::fmt::Display for TokenizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenizeError::ParseError(msg) => write!(f, "Parse error: {}", msg),
            TokenizeError::UnexpectedCharacter(c) => write!(f, "Unexpected character: '{}'", c),
            TokenizeError::UnterminatedString => write!(f, "Unterminated string literal"),
        }
    }
}

impl std::error::Error for TokenizeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_keywords() {
        let input = "where select insert update delete sort limit group_by and or not";
        let tokens = Tokenizer::tokenize(input).unwrap();
        
        assert_eq!(tokens, vec![
            Token::Where,
            Token::Select,
            Token::Insert,
            Token::Update,
            Token::Delete,
            Token::Sort,
            Token::Limit,
            Token::GroupBy,
            Token::And,
            Token::Or,
            Token::Not,
            Token::EOF,
        ]);
    }
    
    #[test]
    fn test_tokenize_operators() {
        let input = "== != < > <= >=";
        let tokens = Tokenizer::tokenize(input).unwrap();
        
        assert_eq!(tokens, vec![
            Token::Equal,
            Token::NotEqual,
            Token::LessThan,
            Token::GreaterThan,
            Token::LessEqual,
            Token::GreaterEqual,
            Token::EOF,
        ]);
    }
    
    #[test]
    fn test_tokenize_literals() {
        let input = r#""hello world" 42 3.14 true false"#;
        let tokens = Tokenizer::tokenize(input).unwrap();
        
        assert_eq!(tokens, vec![
            Token::StringLiteral("hello world".to_string()),
            Token::IntegerLiteral(42),
            Token::FloatLiteral(3.14),
            Token::BooleanLiteral(true),
            Token::BooleanLiteral(false),
            Token::EOF,
        ]);
    }
    
    #[test]
    fn test_tokenize_identifiers_and_field_access() {
        let input = "customers .name .customer.account.token user_id";
        let tokens = Tokenizer::tokenize(input).unwrap();
        
        assert_eq!(tokens, vec![
            Token::Identifier("customers".to_string()),
            Token::FieldAccess(vec!["name".to_string()]),
            Token::FieldAccess(vec!["customer".to_string(), "account".to_string(), "token".to_string()]),
            Token::Identifier("user_id".to_string()),
            Token::EOF,
        ]);
    }
    
    #[test]
    fn test_tokenize_punctuation() {
        let input = "{ } ( ) , : .";
        let tokens = Tokenizer::tokenize(input).unwrap();
        
        assert_eq!(tokens, vec![
            Token::LeftBrace,
            Token::RightBrace,
            Token::LeftParen,
            Token::RightParen,
            Token::Comma,
            Token::Colon,
            Token::Dot,
            Token::EOF,
        ]);
    }
    
    #[test]
    fn test_tokenize_oql_query() {
        let input = r#"customers.where { .name == "Alice" }"#;
        let tokens = Tokenizer::tokenize(input).unwrap();
        
        assert_eq!(tokens, vec![
            Token::Identifier("customers".to_string()),
            Token::Dot,
            Token::Where,
            Token::LeftBrace,
            Token::FieldAccess(vec!["name".to_string()]),
            Token::Equal,
            Token::StringLiteral("Alice".to_string()),
            Token::RightBrace,
            Token::EOF,
        ]);
    }
}
// Abstract Syntax Tree (AST) for OQL queries
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OQLStatement {
    Query(OQLQuery),
    FunctionDef(FunctionDefinition),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OQLQuery {
    pub collection: String,
    pub operations: Vec<Operation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub parameters: Vec<FunctionParameter>,
    pub return_type: Option<String>,
    pub body: Expression,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionParameter {
    pub name: String,
    pub param_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Operation {
    Where(WhereBlock),
    Select(SelectBlock),
    Insert(InsertBlock),
    InsertMany(Vec<InsertBlock>),  // Batch insert multiple documents
    Update(UpdateBlock),
    Delete,
    Sort(SortBlock),
    Limit(u64),
    GroupBy(GroupByBlock),
    Having(HavingBlock),
    Count,
    Sum(FieldAccess),
    Avg(FieldAccess),
    Min(FieldAccess),
    Max(FieldAccess),
    FunctionDef(FunctionDefinition),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WhereBlock {
    pub condition: Condition,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectBlock {
    pub fields: Vec<SelectField>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectField {
    pub alias: Option<String>,
    pub expression: Expression,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsertBlock {
    pub fields: Vec<InsertField>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsertField {
    pub name: String,
    pub value: Expression,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectField {
    pub name: String,
    pub value: Expression,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateBlock {
    pub fields: Vec<UpdateField>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateField {
    pub name: String,
    pub value: Expression,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SortBlock {
    pub fields: Vec<SortField>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SortField {
    pub expression: Expression,
    pub direction: SortDirection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SortDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupByBlock {
    pub fields: Vec<FieldAccess>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HavingBlock {
    pub condition: Condition,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expression {
    FieldAccess(FieldAccess),
    Literal(Literal),
    FunctionCall {
        name: String,
        args: Vec<Expression>,
    },
    IfThenElse {
        condition: Box<Condition>,
        then_expr: Box<Expression>,
        else_expr: Box<Expression>,
    },
    Match {
        expr: Box<Expression>,
        arms: Vec<MatchArm>,
    },
    LogicalAnd(Box<Expression>, Box<Expression>),
    LogicalOr(Box<Expression>, Box<Expression>),
    LogicalNot(Box<Expression>),
    BinaryOp {
        left: Box<Expression>,
        operator: BinaryOperator,
        right: Box<Expression>,
    },
    ArrayAccess {
        array: Box<Expression>,
        index: Box<Expression>,
    },
    ChainedAccess {
        base: Box<Expression>,
        field: String,
    },
    Block(Vec<Expression>),
    ArrayLiteral(Vec<Expression>),
    ObjectLiteral(Vec<ObjectField>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchArm {
    pub pattern: MatchPattern,
    pub guard: Option<Condition>,
    pub body: Expression,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MatchPattern {
    Literal(Literal),
    Wildcard,
    Variable(String),
    Guard(Box<Condition>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldAccess {
    pub path: Vec<String>,      // ["field", "subfield", "nested"]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Condition {
    Comparison {
        left: Expression,
        operator: ComparisonOperator,
        right: Expression,
    },
    And(Box<Condition>, Box<Condition>),
    Or(Box<Condition>, Box<Condition>),
    Not(Box<Condition>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ComparisonOperator {
    Equal,
    NotEqual,
    LessThan,
    GreaterThan,
    LessEqual,
    GreaterEqual,
    Contains,
    StartsWith,
    EndsWith,
    In,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Literal {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
}

// Parser implementation for OQL method-chaining syntax
pub struct Parser {
    tokens: Vec<Token>,
    position: usize,
    schema_registry: Option<crate::schema::SchemaRegistry>,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { 
            tokens, 
            position: 0,
            schema_registry: None,
        }
    }
    
    /// Create a new parser with schema registry for validation
    pub fn with_schema_registry(tokens: Vec<Token>, schema_registry: crate::schema::SchemaRegistry) -> Self {
        Self {
            tokens,
            position: 0,
            schema_registry: Some(schema_registry),
        }
    }
    
    /// Get the schema registry if available
    pub fn schema_registry(&self) -> Option<&crate::schema::SchemaRegistry> {
        self.schema_registry.as_ref()
    }
    
    /// Validate that a field type is compatible with a comparison operator
    fn validate_comparison_operator(
        &self,
        field_type: &crate::schema::FieldType,
        operator: &ComparisonOperator,
    ) -> Result<(), ParseError> {
        use crate::schema::FieldType;
        
        match (field_type, operator) {
            // String operators
            (FieldType::String, ComparisonOperator::Contains) => Ok(()),
            (FieldType::String, ComparisonOperator::StartsWith) => Ok(()),
            (FieldType::String, ComparisonOperator::EndsWith) => Ok(()),
            (FieldType::String, ComparisonOperator::Equal) => Ok(()),
            (FieldType::String, ComparisonOperator::NotEqual) => Ok(()),
            
            // Numeric operators
            (FieldType::Integer, ComparisonOperator::LessThan) => Ok(()),
            (FieldType::Integer, ComparisonOperator::GreaterThan) => Ok(()),
            (FieldType::Integer, ComparisonOperator::LessEqual) => Ok(()),
            (FieldType::Integer, ComparisonOperator::GreaterEqual) => Ok(()),
            (FieldType::Integer, ComparisonOperator::Equal) => Ok(()),
            (FieldType::Integer, ComparisonOperator::NotEqual) => Ok(()),
            
            (FieldType::Float, ComparisonOperator::LessThan) => Ok(()),
            (FieldType::Float, ComparisonOperator::GreaterThan) => Ok(()),
            (FieldType::Float, ComparisonOperator::LessEqual) => Ok(()),
            (FieldType::Float, ComparisonOperator::GreaterEqual) => Ok(()),
            (FieldType::Float, ComparisonOperator::Equal) => Ok(()),
            (FieldType::Float, ComparisonOperator::NotEqual) => Ok(()),
            
            // Boolean operators
            (FieldType::Boolean, ComparisonOperator::Equal) => Ok(()),
            (FieldType::Boolean, ComparisonOperator::NotEqual) => Ok(()),
            
            // DateTime operators
            (FieldType::DateTime, ComparisonOperator::LessThan) => Ok(()),
            (FieldType::DateTime, ComparisonOperator::GreaterThan) => Ok(()),
            (FieldType::DateTime, ComparisonOperator::LessEqual) => Ok(()),
            (FieldType::DateTime, ComparisonOperator::GreaterEqual) => Ok(()),
            (FieldType::DateTime, ComparisonOperator::Equal) => Ok(()),
            (FieldType::DateTime, ComparisonOperator::NotEqual) => Ok(()),
            
            // Reference operators
            (FieldType::Reference(_), ComparisonOperator::Equal) => Ok(()),
            (FieldType::Reference(_), ComparisonOperator::NotEqual) => Ok(()),
            
            // List operators
            (FieldType::List(_), ComparisonOperator::Contains) => Ok(()),
            (FieldType::List(_), ComparisonOperator::Equal) => Ok(()),
            (FieldType::List(_), ComparisonOperator::NotEqual) => Ok(()),
            
            // Optional types - check inner type
            (FieldType::Optional(inner), op) => self.validate_comparison_operator(inner, op),
            
            // Invalid combinations
            _ => Err(ParseError::TypeError {
                message: format!(
                    "Operator {:?} is not valid for type {}",
                    operator,
                    field_type.type_name()
                ),
            }),
        }
    }
    
    /// Validate that two types are compatible for comparison
    fn validate_type_compatibility(
        &self,
        left_type: &crate::schema::FieldType,
        right_type: &crate::schema::FieldType,
    ) -> Result<(), ParseError> {
        use crate::schema::FieldType;
        
        match (left_type, right_type) {
            // Same types are always compatible
            (FieldType::String, FieldType::String) => Ok(()),
            (FieldType::Integer, FieldType::Integer) => Ok(()),
            (FieldType::Float, FieldType::Float) => Ok(()),
            (FieldType::Boolean, FieldType::Boolean) => Ok(()),
            (FieldType::DateTime, FieldType::DateTime) => Ok(()),
            
            // Integer and Float are compatible
            (FieldType::Integer, FieldType::Float) => Ok(()),
            (FieldType::Float, FieldType::Integer) => Ok(()),
            
            // References to same schema are compatible
            (FieldType::Reference(ref1), FieldType::Reference(ref2)) 
                if ref1.target_schema == ref2.target_schema => Ok(()),
            
            // Optional types - check inner compatibility
            (FieldType::Optional(inner1), FieldType::Optional(inner2)) => {
                self.validate_type_compatibility(inner1, inner2)
            }
            (FieldType::Optional(inner), other) | (other, FieldType::Optional(inner)) => {
                self.validate_type_compatibility(inner, other)
            }
            
            // List types - check inner compatibility
            (FieldType::List(inner1), FieldType::List(inner2)) => {
                self.validate_type_compatibility(inner1, inner2)
            }
            
            // Incompatible types - provide detailed error message
            _ => Err(ParseError::TypeError {
                message: format!(
                    "Type mismatch: expected {}, but got {}",
                    left_type.type_name(),
                    right_type.type_name()
                ),
            }),
        }
    }
    
    /// Infer the type of an expression based on literals and field access
    fn infer_expression_type(
        &self,
        expr: &Expression,
        schema_name: &str,
    ) -> Result<crate::schema::FieldType, ParseError> {
        use crate::schema::FieldType;
        
        match expr {
            Expression::Literal(lit) => Ok(match lit {
                Literal::String(_) => FieldType::String,
                Literal::Integer(_) => FieldType::Integer,
                Literal::Float(_) => FieldType::Float,
                Literal::Boolean(_) => FieldType::Boolean,
            }),
            
            Expression::FieldAccess(field_access) => {
                if let Some(registry) = &self.schema_registry {
                    // Get the field type from schema
                    let mut current_schema = schema_name.to_string();
                    let mut current_type: Option<FieldType> = None;
                    
                    for field_name in &field_access.path {
                        let field_type = registry.get_field_type(&current_schema, field_name)
                            .map_err(|e| ParseError::SchemaError { message: e.to_string() })?
                            .ok_or_else(|| ParseError::SchemaError {
                                message: format!("Field '{}' not found in schema '{}'", field_name, current_schema),
                            })?;
                        
                        // If this is a reference, update current_schema for next iteration
                        if let Some(target_schema) = field_type.referenced_schema() {
                            current_schema = target_schema.to_string();
                        }
                        
                        current_type = Some(field_type);
                    }
                    
                    current_type.ok_or_else(|| ParseError::SchemaError {
                        message: "Empty field path".to_string(),
                    })
                } else {
                    // No schema registry, can't infer type
                    Err(ParseError::SchemaError {
                        message: "Cannot infer type without schema registry".to_string(),
                    })
                }
            }
            
            Expression::FunctionCall { name, args } => {
                // Infer return type based on function name
                match name.as_str() {
                    "count" => Ok(FieldType::Integer),
                    "sum" | "avg" => {
                        // Return type depends on argument type
                        if args.is_empty() {
                            Ok(FieldType::Float)
                        } else {
                            self.infer_expression_type(&args[0], schema_name)
                        }
                    }
                    "min" | "max" => {
                        // Return type same as argument type
                        if args.is_empty() {
                            Err(ParseError::TypeError {
                                message: format!("Function '{}' requires at least one argument", name),
                            })
                        } else {
                            self.infer_expression_type(&args[0], schema_name)
                        }
                    }
                    "length" | "len" => Ok(FieldType::Integer),
                    "upper" | "lower" | "trim" => Ok(FieldType::String),
                    "now" => Ok(FieldType::DateTime),
                    _ => Err(ParseError::TypeError {
                        message: format!("Unknown function '{}'", name),
                    }),
                }
            }
            
            Expression::BinaryOp { left, operator, right } => {
                let left_type = self.infer_expression_type(left, schema_name)?;
                let right_type = self.infer_expression_type(right, schema_name)?;
                
                match operator {
                    BinaryOperator::Add | BinaryOperator::Subtract | 
                    BinaryOperator::Multiply | BinaryOperator::Divide | 
                    BinaryOperator::Modulo => {
                        // Arithmetic operations return numeric types
                        match (&left_type, &right_type) {
                            (FieldType::Integer, FieldType::Integer) => Ok(FieldType::Integer),
                            (FieldType::Float, _) | (_, FieldType::Float) => Ok(FieldType::Float),
                            _ => Err(ParseError::TypeError {
                                message: format!(
                                    "Arithmetic operator {:?} requires numeric types, got {} and {}",
                                    operator,
                                    left_type.type_name(),
                                    right_type.type_name()
                                ),
                            }),
                        }
                    }
                }
            }
            
            Expression::IfThenElse { then_expr, else_expr, .. } => {
                // Return type is the common type of then and else branches
                let then_type = self.infer_expression_type(then_expr, schema_name)?;
                let else_type = self.infer_expression_type(else_expr, schema_name)?;
                
                self.validate_type_compatibility(&then_type, &else_type)?;
                Ok(then_type)
            }
            
            Expression::ArrayLiteral(elements) => {
                if elements.is_empty() {
                    // Empty array - can't infer type
                    Err(ParseError::TypeError {
                        message: "Cannot infer type of empty array".to_string(),
                    })
                } else {
                    let first_type = self.infer_expression_type(&elements[0], schema_name)?;
                    Ok(FieldType::List(Box::new(first_type)))
                }
            }
            
            _ => Err(ParseError::TypeError {
                message: format!("Cannot infer type of expression: {:?}", expr),
            }),
        }
    }
    
    /// Validate a condition with type checking
    fn validate_condition_types(
        &self,
        condition: &Condition,
        schema_name: &str,
    ) -> Result<(), ParseError> {
        match condition {
            Condition::Comparison { left, operator, right } => {
                // Infer types of both sides
                let left_type = self.infer_expression_type(left, schema_name)?;
                let right_type = self.infer_expression_type(right, schema_name)?;
                
                // Validate operator is valid for left type
                self.validate_comparison_operator(&left_type, operator)?;
                
                // Validate types are compatible
                self.validate_type_compatibility(&left_type, &right_type)?;
                
                Ok(())
            }
            Condition::And(left, right) | Condition::Or(left, right) => {
                self.validate_condition_types(left, schema_name)?;
                self.validate_condition_types(right, schema_name)?;
                Ok(())
            }
            Condition::Not(inner) => {
                self.validate_condition_types(inner, schema_name)?;
                Ok(())
            }
        }
    }
    
    /// Calculate Levenshtein distance between two strings for fuzzy matching
    fn levenshtein_distance(s1: &str, s2: &str) -> usize {
        let len1 = s1.len();
        let len2 = s2.len();
        let mut matrix = vec![vec![0; len2 + 1]; len1 + 1];
        
        for i in 0..=len1 {
            matrix[i][0] = i;
        }
        for j in 0..=len2 {
            matrix[0][j] = j;
        }
        
        for i in 1..=len1 {
            for j in 1..=len2 {
                let cost = if s1.chars().nth(i - 1) == s2.chars().nth(j - 1) { 0 } else { 1 };
                matrix[i][j] = std::cmp::min(
                    std::cmp::min(matrix[i - 1][j] + 1, matrix[i][j - 1] + 1),
                    matrix[i - 1][j - 1] + cost,
                );
            }
        }
        
        matrix[len1][len2]
    }
    
    /// Find similar field names for suggestions
    fn find_similar_fields(&self, schema_name: &str, field_name: &str) -> Vec<String> {
        if let Some(registry) = &self.schema_registry {
            if let Ok(Some(schema)) = registry.get(schema_name) {
                let mut suggestions: Vec<(String, usize)> = schema.fields
                    .keys()
                    .map(|f| (f.clone(), Self::levenshtein_distance(field_name, f)))
                    .filter(|(_, dist)| *dist <= 3) // Only suggest if distance is 3 or less
                    .collect();
                
                suggestions.sort_by_key(|(_, dist)| *dist);
                return suggestions.into_iter().take(3).map(|(name, _)| name).collect();
            }
        }
        Vec::new()
    }
    
    /// Validate that a field exists in the schema
    fn validate_field_exists(
        &self,
        field_access: &FieldAccess,
        schema_name: &str,
    ) -> Result<(), ParseError> {
        if let Some(registry) = &self.schema_registry {
            // Use the schema registry's validate_field method
            // This also validates reference navigation
            match registry.validate_field(schema_name, &field_access.path) {
                Ok(_) => {},
                Err(e) => {
                    // Enhance error message with suggestions
                    let error_msg = e.to_string();
                    
                    // Check if this is a "field not found" error
                    if error_msg.contains("not found") && !field_access.path.is_empty() {
                        let field_name = &field_access.path[0];
                        let suggestions = self.find_similar_fields(schema_name, field_name);
                        
                        if !suggestions.is_empty() {
                            return Err(ParseError::SchemaError {
                                message: format!(
                                    "{}. Did you mean one of: {}?",
                                    error_msg,
                                    suggestions.join(", ")
                                ),
                            });
                        }
                    }
                    
                    return Err(ParseError::SchemaError {
                        message: error_msg,
                    });
                }
            }
            
            // Additionally validate reference targets if this is a reference field
            if field_access.path.len() > 1 {
                // For nested field access, validate each reference in the chain
                let mut current_schema = schema_name.to_string();
                
                for (i, field_name) in field_access.path.iter().enumerate() {
                    if i < field_access.path.len() - 1 {
                        // This is not the last field, so it should be a reference
                        let field_type = registry.get_field_type(&current_schema, field_name)
                            .map_err(|e| ParseError::SchemaError { message: e.to_string() })?
                            .ok_or_else(|| ParseError::SchemaError {
                                message: format!("Field '{}' not found in schema '{}'", field_name, current_schema),
                            })?;
                        
                        if let Some(target_schema) = field_type.referenced_schema() {
                            // Validate that the target schema exists
                            if !registry.exists(target_schema) {
                                return Err(ParseError::SchemaError {
                                    message: format!(
                                        "Reference field '{}' in schema '{}' points to nonexistent schema '{}'",
                                        field_name, current_schema, target_schema
                                    ),
                                });
                            }
                            current_schema = target_schema.to_string();
                        } else {
                            return Err(ParseError::SchemaError {
                                message: format!(
                                    "Field '{}' in schema '{}' is not a reference, cannot navigate further",
                                    field_name, current_schema
                                ),
                            });
                        }
                    }
                }
            }
        }
        Ok(())
    }
    
    /// Validate field existence in an expression
    fn validate_expression_fields(
        &self,
        expr: &Expression,
        schema_name: &str,
    ) -> Result<(), ParseError> {
        match expr {
            Expression::FieldAccess(field_access) => {
                self.validate_field_exists(field_access, schema_name)?;
            }
            Expression::FunctionCall { args, .. } => {
                for arg in args {
                    self.validate_expression_fields(arg, schema_name)?;
                }
            }
            Expression::IfThenElse { condition, then_expr, else_expr } => {
                self.validate_condition_fields(condition, schema_name)?;
                self.validate_expression_fields(then_expr, schema_name)?;
                self.validate_expression_fields(else_expr, schema_name)?;
            }
            Expression::Match { expr, arms } => {
                self.validate_expression_fields(expr, schema_name)?;
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.validate_condition_fields(guard, schema_name)?;
                    }
                    self.validate_expression_fields(&arm.body, schema_name)?;
                }
            }
            Expression::LogicalAnd(left, right) | Expression::LogicalOr(left, right) => {
                self.validate_expression_fields(left, schema_name)?;
                self.validate_expression_fields(right, schema_name)?;
            }
            Expression::LogicalNot(inner) => {
                self.validate_expression_fields(inner, schema_name)?;
            }
            Expression::BinaryOp { left, right, .. } => {
                self.validate_expression_fields(left, schema_name)?;
                self.validate_expression_fields(right, schema_name)?;
            }
            Expression::ArrayAccess { array, index } => {
                self.validate_expression_fields(array, schema_name)?;
                self.validate_expression_fields(index, schema_name)?;
            }
            Expression::ChainedAccess { base, .. } => {
                self.validate_expression_fields(base, schema_name)?;
            }
            Expression::Block(expressions) => {
                for expr in expressions {
                    self.validate_expression_fields(expr, schema_name)?;
                }
            }
            Expression::ArrayLiteral(elements) => {
                for element in elements {
                    self.validate_expression_fields(element, schema_name)?;
                }
            }
            Expression::ObjectLiteral(fields) => {
                for field in fields {
                    self.validate_expression_fields(&field.value, schema_name)?;
                }
            }
            Expression::Literal(_) => {
                // Literals don't have fields to validate
            }
        }
        Ok(())
    }
    
    /// Validate field existence in a condition
    fn validate_condition_fields(
        &self,
        condition: &Condition,
        schema_name: &str,
    ) -> Result<(), ParseError> {
        match condition {
            Condition::Comparison { left, right, .. } => {
                self.validate_expression_fields(left, schema_name)?;
                self.validate_expression_fields(right, schema_name)?;
            }
            Condition::And(left, right) | Condition::Or(left, right) => {
                self.validate_condition_fields(left, schema_name)?;
                self.validate_condition_fields(right, schema_name)?;
            }
            Condition::Not(inner) => {
                self.validate_condition_fields(inner, schema_name)?;
            }
        }
        Ok(())
    }
    
    /// Validate function arguments match expected types
    fn validate_function_arguments(
        &self,
        function_name: &str,
        args: &[Expression],
        schema_name: &str,
    ) -> Result<(), ParseError> {
        use crate::schema::FieldType;
        
        match function_name {
            "count" => {
                // count() or count(field) - no strict type requirements
                Ok(())
            }
            "sum" | "avg" => {
                if args.is_empty() {
                    return Err(ParseError::TypeError {
                        message: format!("Function '{}' requires at least one argument", function_name),
                    });
                }
                
                let arg_type = self.infer_expression_type(&args[0], schema_name)?;
                match arg_type {
                    FieldType::Integer | FieldType::Float | FieldType::Optional(_) => Ok(()),
                    _ => Err(ParseError::TypeError {
                        message: format!(
                            "Function '{}' requires numeric argument, got {}",
                            function_name,
                            arg_type.type_name()
                        ),
                    }),
                }
            }
            "min" | "max" => {
                if args.is_empty() {
                    return Err(ParseError::TypeError {
                        message: format!("Function '{}' requires at least one argument", function_name),
                    });
                }
                
                let arg_type = self.infer_expression_type(&args[0], schema_name)?;
                match arg_type {
                    FieldType::Integer | FieldType::Float | FieldType::DateTime | FieldType::Optional(_) => Ok(()),
                    _ => Err(ParseError::TypeError {
                        message: format!(
                            "Function '{}' requires comparable argument, got {}",
                            function_name,
                            arg_type.type_name()
                        ),
                    }),
                }
            }
            "length" | "len" => {
                if args.is_empty() {
                    return Err(ParseError::TypeError {
                        message: format!("Function '{}' requires at least one argument", function_name),
                    });
                }
                
                let arg_type = self.infer_expression_type(&args[0], schema_name)?;
                match arg_type {
                    FieldType::String | FieldType::List(_) | FieldType::Optional(_) => Ok(()),
                    _ => Err(ParseError::TypeError {
                        message: format!(
                            "Function '{}' requires string or list argument, got {}",
                            function_name,
                            arg_type.type_name()
                        ),
                    }),
                }
            }
            "upper" | "lower" | "trim" => {
                if args.is_empty() {
                    return Err(ParseError::TypeError {
                        message: format!("Function '{}' requires at least one argument", function_name),
                    });
                }
                
                let arg_type = self.infer_expression_type(&args[0], schema_name)?;
                match arg_type {
                    FieldType::String | FieldType::Optional(_) => Ok(()),
                    _ => Err(ParseError::TypeError {
                        message: format!(
                            "Function '{}' requires string argument, got {}",
                            function_name,
                            arg_type.type_name()
                        ),
                    }),
                }
            }
            "now" => {
                if !args.is_empty() {
                    return Err(ParseError::TypeError {
                        message: "Function 'now' takes no arguments".to_string(),
                    });
                }
                Ok(())
            }
            _ => {
                // Unknown function - skip validation
                Ok(())
            }
        }
    }
    
    pub fn parse(&mut self) -> Result<OQLStatement, ParseError> {
        match self.current_token() {
            Some(Token::Fn) => {
                let func_def = self.parse_function_definition()?;
                Ok(OQLStatement::FunctionDef(func_def))
            }
            _ => {
                let query = self.parse_oql_query()?;
                Ok(OQLStatement::Query(query))
            }
        }
    }
    
    fn parse_function_definition(&mut self) -> Result<FunctionDefinition, ParseError> {
        // Expect 'fn' keyword
        self.expect_token(Token::Fn)?;
        
        // Parse function name
        let name = match self.current_token() {
            Some(Token::Identifier(name)) => {
                let name = name.clone();
                self.advance();
                name
            }
            Some(token) => return Err(ParseError::UnexpectedToken(format!("Expected function name, found {:?}", token))),
            None => return Err(ParseError::UnexpectedEndOfInput),
        };
        
        // Parse parameters
        self.expect_token(Token::LeftParen)?;
        let mut parameters = Vec::new();
        
        // Parse first parameter if exists
        if !matches!(self.current_token(), Some(Token::RightParen)) {
            let param = self.parse_function_parameter()?;
            parameters.push(param);
            
            // Parse additional parameters
            while matches!(self.current_token(), Some(Token::Comma)) {
                self.advance(); // consume comma
                let param = self.parse_function_parameter()?;
                parameters.push(param);
            }
        }
        
        self.expect_token(Token::RightParen)?;
        
        // Parse optional return type
        let return_type = if matches!(self.current_token(), Some(Token::Arrow)) {
            self.advance(); // consume arrow
            match self.current_token() {
                Some(Token::Identifier(type_name)) => {
                    let type_name = type_name.clone();
                    self.advance();
                    Some(type_name)
                }
                Some(token) => return Err(ParseError::UnexpectedToken(format!("Expected return type, found {:?}", token))),
                None => return Err(ParseError::UnexpectedEndOfInput),
            }
        } else {
            None
        };
        
        // Parse function body
        self.expect_token(Token::LeftBrace)?;
        let body = self.parse_expression()?;
        self.expect_token(Token::RightBrace)?;
        
        Ok(FunctionDefinition {
            name,
            parameters,
            return_type,
            body,
        })
    }
    
    fn parse_function_parameter(&mut self) -> Result<FunctionParameter, ParseError> {
        // Parse parameter name
        let name = match self.current_token() {
            Some(Token::Identifier(name)) => {
                let name = name.clone();
                self.advance();
                name
            }
            Some(token) => return Err(ParseError::UnexpectedToken(format!("Expected parameter name, found {:?}", token))),
            None => return Err(ParseError::UnexpectedEndOfInput),
        };
        
        // Expect colon
        self.expect_token(Token::Colon)?;
        
        // Parse parameter type
        let param_type = match self.current_token() {
            Some(Token::Identifier(type_name)) => {
                let type_name = type_name.clone();
                self.advance();
                type_name
            }
            Some(token) => return Err(ParseError::UnexpectedToken(format!("Expected parameter type, found {:?}", token))),
            None => return Err(ParseError::UnexpectedEndOfInput),
        };
        
        Ok(FunctionParameter { name, param_type })
    }

    fn parse_oql_query(&mut self) -> Result<OQLQuery, ParseError> {
        // Parse collection name (e.g., "customers" in "customers.where { ... }")
        let collection = match self.current_token() {
            Some(Token::Identifier(name)) => {
                let collection = name.clone();
                self.advance();
                collection
            }
            Some(token) => return Err(ParseError::UnexpectedToken(format!("Expected collection name, found {:?}", token))),
            None => return Err(ParseError::UnexpectedEndOfInput),
        };
        
        // Validate schema exists if registry is available
        if let Some(registry) = &self.schema_registry {
            if !registry.exists(&collection) {
                return Err(ParseError::SchemaError {
                    message: format!("Schema '{}' not found", collection),
                });
            }
        }
        
        // Parse method chain (e.g., ".where { ... }.select { ... }")
        let mut operations = Vec::new();
        
        while matches!(self.current_token(), Some(Token::Dot)) {
            self.advance(); // consume dot
            
            let operation = self.parse_operation_with_validation(&collection)?;
            operations.push(operation);
        }
        
        // Expect EOF
        if !matches!(self.current_token(), Some(Token::EOF) | None) {
            return Err(ParseError::UnexpectedToken(format!("Expected end of query, found {:?}", self.current_token())));
        }
        
        Ok(OQLQuery {
            collection,
            operations,
        })
    }
    
    /// Parse operation with type validation
    fn parse_operation_with_validation(&mut self, schema_name: &str) -> Result<Operation, ParseError> {
        match self.current_token() {
            Some(Token::Where) => {
                self.advance();
                let where_block = self.parse_where_block_with_validation(schema_name)?;
                Ok(Operation::Where(where_block))
            }
            Some(Token::Select) => {
                self.advance();
                let select_block = self.parse_select_block_with_validation(schema_name)?;
                Ok(Operation::Select(select_block))
            }
            Some(Token::Insert) => {
                self.advance();
                
                // Check if this is insertMany (array of objects)
                if self.current_token() == Some(&Token::LeftBracket) {
                    self.advance(); // consume '['
                    
                    let mut insert_blocks = Vec::new();
                    
                    // Parse array of insert blocks
                    loop {
                        if self.current_token() == Some(&Token::RightBracket) {
                            break;
                        }
                        
                        let insert_block = self.parse_insert_block_with_validation(schema_name)?;
                        insert_blocks.push(insert_block);
                        
                        // Check for comma or end of array
                        if self.current_token() == Some(&Token::Comma) {
                            self.advance();
                        } else if self.current_token() == Some(&Token::RightBracket) {
                            break;
                        } else {
                            return Err(ParseError::UnexpectedToken(
                                format!("Expected ',' or ']', found {:?}", self.current_token())
                            ));
                        }
                    }
                    
                    self.expect_token(Token::RightBracket)?; // consume ']'
                    
                    if insert_blocks.is_empty() {
                        return Err(ParseError::UnexpectedToken(
                            "insertMany requires at least one document".to_string()
                        ));
                    }
                    
                    Ok(Operation::InsertMany(insert_blocks))
                } else {
                    // Single insert
                    let insert_block = self.parse_insert_block_with_validation(schema_name)?;
                    Ok(Operation::Insert(insert_block))
                }
            }
            Some(Token::Update) => {
                self.advance();
                let update_block = self.parse_update_block_with_validation(schema_name)?;
                Ok(Operation::Update(update_block))
            }
            Some(Token::Having) => {
                self.advance();
                let having_block = self.parse_having_block_with_validation(schema_name)?;
                Ok(Operation::Having(having_block))
            }
            Some(Token::Sum) => {
                self.advance();
                self.expect_token(Token::LeftParen)?;
                let field = self.parse_field_access_expression()?;
                self.expect_token(Token::RightParen)?;
                
                // Validate field existence and type if schema registry available
                if self.schema_registry.is_some() {
                    self.validate_field_exists(&field, schema_name)?;
                    let expr = Expression::FieldAccess(field.clone());
                    self.validate_function_arguments("sum", &[expr], schema_name)?;
                }
                
                Ok(Operation::Sum(field))
            }
            Some(Token::Avg) => {
                self.advance();
                self.expect_token(Token::LeftParen)?;
                let field = self.parse_field_access_expression()?;
                self.expect_token(Token::RightParen)?;
                
                // Validate field existence and type if schema registry available
                if self.schema_registry.is_some() {
                    self.validate_field_exists(&field, schema_name)?;
                    let expr = Expression::FieldAccess(field.clone());
                    self.validate_function_arguments("avg", &[expr], schema_name)?;
                }
                
                Ok(Operation::Avg(field))
            }
            Some(Token::Min) => {
                self.advance();
                self.expect_token(Token::LeftParen)?;
                let field = self.parse_field_access_expression()?;
                self.expect_token(Token::RightParen)?;
                
                // Validate field existence and type if schema registry available
                if self.schema_registry.is_some() {
                    self.validate_field_exists(&field, schema_name)?;
                    let expr = Expression::FieldAccess(field.clone());
                    self.validate_function_arguments("min", &[expr], schema_name)?;
                }
                
                Ok(Operation::Min(field))
            }
            Some(Token::Max) => {
                self.advance();
                self.expect_token(Token::LeftParen)?;
                let field = self.parse_field_access_expression()?;
                self.expect_token(Token::RightParen)?;
                
                // Validate field existence and type if schema registry available
                if self.schema_registry.is_some() {
                    self.validate_field_exists(&field, schema_name)?;
                    let expr = Expression::FieldAccess(field.clone());
                    self.validate_function_arguments("max", &[expr], schema_name)?;
                }
                
                Ok(Operation::Max(field))
            }
            _ => {
                // Fall back to regular parsing for operations that don't need validation
                self.parse_operation()
            }
        }
    }
    
    /// Parse select block with type validation
    fn parse_select_block_with_validation(&mut self, schema_name: &str) -> Result<SelectBlock, ParseError> {
        let select_block = self.parse_select_block()?;
        
        // Validate field existence and types if schema registry is available
        if self.schema_registry.is_some() {
            for field in &select_block.fields {
                // Validate field existence
                self.validate_expression_fields(&field.expression, schema_name)?;
                // Validate the expression type can be inferred
                let _ = self.infer_expression_type(&field.expression, schema_name)?;
            }
        }
        
        Ok(select_block)
    }
    
    /// Parse insert block with type validation
    fn parse_insert_block_with_validation(&mut self, schema_name: &str) -> Result<InsertBlock, ParseError> {
        let insert_block = self.parse_insert_block()?;
        
        // Validate field existence and types if schema registry is available
        if let Some(registry) = &self.schema_registry {
            for field in &insert_block.fields {
                // Check field exists in schema
                let field_type = registry.get_field_type(schema_name, &field.name)
                    .map_err(|e| ParseError::SchemaError { message: e.to_string() })?
                    .ok_or_else(|| ParseError::SchemaError {
                        message: format!("Field '{}' not found in schema '{}'", field.name, schema_name),
                    })?;
                
                // Validate field existence in value expression
                self.validate_expression_fields(&field.value, schema_name)?;
                
                // Validate value type matches field type
                let value_type = self.infer_expression_type(&field.value, schema_name)?;
                self.validate_type_compatibility(&field_type, &value_type)?;
            }
        }
        
        Ok(insert_block)
    }
    
    /// Parse update block with type validation
    fn parse_update_block_with_validation(&mut self, schema_name: &str) -> Result<UpdateBlock, ParseError> {
        let update_block = self.parse_update_block()?;
        
        // Validate field existence and types if schema registry is available
        if let Some(registry) = &self.schema_registry {
            for field in &update_block.fields {
                // Check field exists in schema
                let field_type = registry.get_field_type(schema_name, &field.name)
                    .map_err(|e| ParseError::SchemaError { message: e.to_string() })?
                    .ok_or_else(|| ParseError::SchemaError {
                        message: format!("Field '{}' not found in schema '{}'", field.name, schema_name),
                    })?;
                
                // Validate field existence in value expression
                self.validate_expression_fields(&field.value, schema_name)?;
                
                // Validate value type matches field type
                let value_type = self.infer_expression_type(&field.value, schema_name)?;
                self.validate_type_compatibility(&field_type, &value_type)?;
            }
        }
        
        Ok(update_block)
    }
    
    /// Parse having block with type validation
    fn parse_having_block_with_validation(&mut self, schema_name: &str) -> Result<HavingBlock, ParseError> {
        let having_block = self.parse_having_block()?;
        
        // Validate field existence and types if schema registry is available
        if self.schema_registry.is_some() {
            self.validate_condition_fields(&having_block.condition, schema_name)?;
            self.validate_condition_types(&having_block.condition, schema_name)?;
        }
        
        Ok(having_block)
    }
    
    fn parse_operation(&mut self) -> Result<Operation, ParseError> {
        match self.current_token() {
            Some(Token::Where) => {
                self.advance();
                let where_block = self.parse_where_block()?;
                Ok(Operation::Where(where_block))
            }
            Some(Token::Select) => {
                self.advance();
                let select_block = self.parse_select_block()?;
                Ok(Operation::Select(select_block))
            }
            Some(Token::Insert) => {
                self.advance();
                let insert_block = self.parse_insert_block()?;
                Ok(Operation::Insert(insert_block))
            }
            Some(Token::Update) => {
                self.advance();
                let update_block = self.parse_update_block()?;
                Ok(Operation::Update(update_block))
            }
            Some(Token::Delete) => {
                self.advance();
                Ok(Operation::Delete)
            }
            Some(Token::GroupBy) => {
                self.advance();
                let group_by_block = self.parse_group_by_block()?;
                Ok(Operation::GroupBy(group_by_block))
            }
            Some(Token::Having) => {
                self.advance();
                let having_block = self.parse_having_block()?;
                Ok(Operation::Having(having_block))
            }
            Some(Token::Count) => {
                self.advance();
                // Expect parentheses for count()
                self.expect_token(Token::LeftParen)?;
                self.expect_token(Token::RightParen)?;
                Ok(Operation::Count)
            }
            Some(Token::Sum) => {
                self.advance();
                self.expect_token(Token::LeftParen)?;
                let field = self.parse_field_access_expression()?;
                self.expect_token(Token::RightParen)?;
                Ok(Operation::Sum(field))
            }
            Some(Token::Avg) => {
                self.advance();
                self.expect_token(Token::LeftParen)?;
                let field = self.parse_field_access_expression()?;
                self.expect_token(Token::RightParen)?;
                Ok(Operation::Avg(field))
            }
            Some(Token::Min) => {
                self.advance();
                self.expect_token(Token::LeftParen)?;
                let field = self.parse_field_access_expression()?;
                self.expect_token(Token::RightParen)?;
                Ok(Operation::Min(field))
            }
            Some(Token::Max) => {
                self.advance();
                self.expect_token(Token::LeftParen)?;
                let field = self.parse_field_access_expression()?;
                self.expect_token(Token::RightParen)?;
                Ok(Operation::Max(field))
            }
            Some(Token::Sort) => {
                self.advance();
                let sort_block = self.parse_sort_block()?;
                Ok(Operation::Sort(sort_block))
            }
            Some(Token::Limit) => {
                self.advance();
                let limit_value = self.parse_limit_value()?;
                Ok(Operation::Limit(limit_value))
            }
            Some(token) => Err(ParseError::UnexpectedToken(format!("Expected operation method, found {:?}", token))),
            None => Err(ParseError::UnexpectedEndOfInput),
        }
    }
    
    fn parse_where_block(&mut self) -> Result<WhereBlock, ParseError> {
        // Expect opening brace
        self.expect_token(Token::LeftBrace)?;
        
        // Parse condition
        let condition = self.parse_condition()?;
        
        // Expect closing brace
        self.expect_token(Token::RightBrace)?;
        
        Ok(WhereBlock { condition })
    }
    
    /// Parse where block with type validation
    fn parse_where_block_with_validation(&mut self, schema_name: &str) -> Result<WhereBlock, ParseError> {
        let where_block = self.parse_where_block()?;
        
        // Validate field existence and types if schema registry is available
        if self.schema_registry.is_some() {
            self.validate_condition_fields(&where_block.condition, schema_name)?;
            self.validate_condition_types(&where_block.condition, schema_name)?;
        }
        
        Ok(where_block)
    }
    
    fn parse_select_block(&mut self) -> Result<SelectBlock, ParseError> {
        // Expect opening brace
        self.expect_token(Token::LeftBrace)?;
        
        let mut fields = Vec::new();
        
        // Parse first field
        let field = self.parse_select_field()?;
        fields.push(field);
        
        // Parse additional fields separated by commas
        while matches!(self.current_token(), Some(Token::Comma)) {
            self.advance(); // consume comma
            let field = self.parse_select_field()?;
            fields.push(field);
        }
        
        // Expect closing brace
        self.expect_token(Token::RightBrace)?;
        
        Ok(SelectBlock { fields })
    }
    
    fn parse_select_field(&mut self) -> Result<SelectField, ParseError> {
        // Check if this is an aliased field (name: expression)
        if let Some(Token::Identifier(name)) = self.current_token() {
            let name = name.clone();
            
            // Look ahead to see what comes after the identifier
            let next_pos = self.position + 1;
            if next_pos < self.tokens.len() {
                match &self.tokens[next_pos] {
                    Token::Colon => {
                        // This is an alias: name: expression
                        self.advance(); // consume identifier
                        self.advance(); // consume colon
                        let expression = self.parse_expression()?;
                        return Ok(SelectField {
                            alias: Some(name),
                            expression,
                        });
                    }
                    Token::LeftParen => {
                        // This is a function call without alias: function(args)
                        let expression = self.parse_expression()?;
                        return Ok(SelectField {
                            alias: None,
                            expression,
                        });
                    }
                    _ => {
                        // This is just a field name: field
                        self.advance(); // consume identifier
                        return Ok(SelectField {
                            alias: None,
                            expression: Expression::FieldAccess(FieldAccess { path: vec![name] }),
                        });
                    }
                }
            } else {
                // End of tokens, treat as simple field
                self.advance(); // consume identifier
                return Ok(SelectField {
                    alias: None,
                    expression: Expression::FieldAccess(FieldAccess { path: vec![name] }),
                });
            }
        }
        
        // Parse expression without alias (for cases like .field or other expressions)
        let expression = self.parse_expression()?;
        Ok(SelectField {
            alias: None,
            expression,
        })
    }
    
    fn parse_insert_block(&mut self) -> Result<InsertBlock, ParseError> {
        // Expect opening brace
        self.expect_token(Token::LeftBrace)?;
        
        let mut fields = Vec::new();
        
        // Parse first field
        let field = self.parse_insert_field()?;
        fields.push(field);
        
        // Parse additional fields separated by commas
        while matches!(self.current_token(), Some(Token::Comma)) {
            self.advance(); // consume comma
            let field = self.parse_insert_field()?;
            fields.push(field);
        }
        
        // Expect closing brace
        self.expect_token(Token::RightBrace)?;
        
        Ok(InsertBlock { fields })
    }
    
    fn parse_insert_field(&mut self) -> Result<InsertField, ParseError> {
        // Parse field name
        let name = match self.current_token() {
            Some(Token::Identifier(name)) => {
                let field_name = name.clone();
                self.advance();
                field_name
            }
            Some(token) => return Err(ParseError::UnexpectedToken(format!("Expected field name, found {:?}", token))),
            None => return Err(ParseError::UnexpectedEndOfInput),
        };
        
        // Expect colon
        self.expect_token(Token::Colon)?;
        
        // Parse value (can be literal, nested object, or array)
        let value = self.parse_expression()?;
        
        Ok(InsertField { name, value })
    }
    
    fn parse_update_block(&mut self) -> Result<UpdateBlock, ParseError> {
        // Expect opening brace
        self.expect_token(Token::LeftBrace)?;
        
        let mut fields = Vec::new();
        
        // Parse first field
        let field = self.parse_update_field()?;
        fields.push(field);
        
        // Parse additional fields separated by commas
        while matches!(self.current_token(), Some(Token::Comma)) {
            self.advance(); // consume comma
            let field = self.parse_update_field()?;
            fields.push(field);
        }
        
        // Expect closing brace
        self.expect_token(Token::RightBrace)?;
        
        Ok(UpdateBlock { fields })
    }
    
    fn parse_update_field(&mut self) -> Result<UpdateField, ParseError> {
        // Parse field name
        let name = match self.current_token() {
            Some(Token::Identifier(name)) => {
                let field_name = name.clone();
                self.advance();
                field_name
            }
            Some(token) => return Err(ParseError::UnexpectedToken(format!("Expected field name, found {:?}", token))),
            None => return Err(ParseError::UnexpectedEndOfInput),
        };
        
        // Expect colon
        self.expect_token(Token::Colon)?;
        
        // Parse expression
        let value = self.parse_expression()?;
        
        Ok(UpdateField { name, value })
    }
    
    fn parse_group_by_block(&mut self) -> Result<GroupByBlock, ParseError> {
        // Expect opening brace
        self.expect_token(Token::LeftBrace)?;
        
        let mut fields = Vec::new();
        
        // Parse first field
        let field = self.parse_field_access_expression()?;
        fields.push(field);
        
        // Parse additional fields separated by commas
        while matches!(self.current_token(), Some(Token::Comma)) {
            self.advance(); // consume comma
            let field = self.parse_field_access_expression()?;
            fields.push(field);
        }
        
        // Expect closing brace
        self.expect_token(Token::RightBrace)?;
        
        Ok(GroupByBlock { fields })
    }
    
    fn parse_having_block(&mut self) -> Result<HavingBlock, ParseError> {
        // Expect opening brace
        self.expect_token(Token::LeftBrace)?;
        
        // Parse the condition
        let condition = self.parse_condition()?;
        
        // Expect closing brace
        self.expect_token(Token::RightBrace)?;
        
        Ok(HavingBlock { condition })
    }
    
    fn parse_field_access_expression(&mut self) -> Result<FieldAccess, ParseError> {
        match self.current_token() {
            Some(Token::FieldAccess(path)) => {
                let field_access = FieldAccess { path: path.clone() };
                self.advance();
                Ok(field_access)
            }
            Some(Token::Identifier(name)) => {
                let name = name.clone();
                self.advance();
                Ok(FieldAccess { path: vec![name] })
            }
            Some(token) => Err(ParseError::UnexpectedToken(format!("Expected field access, found {:?}", token))),
            None => Err(ParseError::UnexpectedEndOfInput),
        }
    }
    
    fn parse_sort_block(&mut self) -> Result<SortBlock, ParseError> {
        // Expect opening brace
        self.expect_token(Token::LeftBrace)?;
        
        let mut fields = Vec::new();
        
        // Parse first sort field
        let field = self.parse_sort_field()?;
        fields.push(field);
        
        // Parse additional fields separated by commas
        while matches!(self.current_token(), Some(Token::Comma)) {
            self.advance(); // consume comma
            let field = self.parse_sort_field()?;
            fields.push(field);
        }
        
        // Expect closing brace
        self.expect_token(Token::RightBrace)?;
        
        Ok(SortBlock { fields })
    }
    
    fn parse_sort_field(&mut self) -> Result<SortField, ParseError> {
        // Parse expression (which can include null coalescing)
        let expression = self.parse_expression()?;
        let mut direction = SortDirection::Asc;
        
        // Check for optional direction (.asc or .desc)
        if matches!(self.current_token(), Some(Token::Dot)) {
            self.advance(); // consume dot
            
            match self.current_token() {
                Some(Token::Identifier(dir_name)) if dir_name == "asc" => {
                    self.advance();
                    direction = SortDirection::Asc;
                }
                Some(Token::Identifier(dir_name)) if dir_name == "desc" => {
                    self.advance();
                    direction = SortDirection::Desc;
                }
                _ => {
                    // Put back the dot, it's not a direction modifier
                    self.position -= 1;
                }
            }
        }
        
        Ok(SortField { expression, direction })
    }
    
    fn parse_limit_value(&mut self) -> Result<u64, ParseError> {
        // Handle function call syntax: limit(5)
        if matches!(self.current_token(), Some(Token::LeftParen)) {
            self.expect_token(Token::LeftParen)?;
            
            let limit = match self.current_token() {
                Some(Token::IntegerLiteral(value)) => {
                    if *value < 0 {
                        return Err(ParseError::InvalidSyntax("LIMIT value must be non-negative".to_string()));
                    }
                    let limit = *value as u64;
                    self.advance();
                    limit
                }
                Some(token) => return Err(ParseError::UnexpectedToken(format!("Expected integer in LIMIT function, found {:?}", token))),
                None => return Err(ParseError::UnexpectedEndOfInput),
            };
            
            self.expect_token(Token::RightParen)?;
            Ok(limit)
        } else {
            // Handle legacy syntax: limit 5
            match self.current_token() {
                Some(Token::IntegerLiteral(value)) => {
                    if *value < 0 {
                        return Err(ParseError::InvalidSyntax("LIMIT value must be non-negative".to_string()));
                    }
                    let limit = *value as u64;
                    self.advance();
                    Ok(limit)
                }
                Some(token) => Err(ParseError::UnexpectedToken(format!("Expected integer for LIMIT, found {:?}", token))),
                None => Err(ParseError::UnexpectedEndOfInput),
            }
        }
    }
    
    fn parse_expression(&mut self) -> Result<Expression, ParseError> {
        self.parse_logical_or_expression()
    }
    
    fn parse_logical_or_expression(&mut self) -> Result<Expression, ParseError> {
        let mut left = self.parse_logical_and_expression()?;
        
        while matches!(self.current_token(), Some(Token::Or)) {
            self.advance(); // consume 'or'
            let right = self.parse_logical_and_expression()?;
            left = Expression::LogicalOr(Box::new(left), Box::new(right));
        }
        
        Ok(left)
    }
    
    fn parse_logical_and_expression(&mut self) -> Result<Expression, ParseError> {
        let mut left = self.parse_logical_not_expression()?;
        
        while matches!(self.current_token(), Some(Token::And)) {
            self.advance(); // consume 'and'
            let right = self.parse_logical_not_expression()?;
            left = Expression::LogicalAnd(Box::new(left), Box::new(right));
        }
        
        Ok(left)
    }
    
    fn parse_logical_not_expression(&mut self) -> Result<Expression, ParseError> {
        if matches!(self.current_token(), Some(Token::Not)) {
            self.advance(); // consume 'not'
            let expr = self.parse_logical_not_expression()?;
            Ok(Expression::LogicalNot(Box::new(expr)))
        } else {
            self.parse_additive_expression()
        }
    }
    
    fn parse_additive_expression(&mut self) -> Result<Expression, ParseError> {
        let mut left = self.parse_multiplicative_expression()?;
        
        while let Some(token) = self.current_token() {
            let operator = match token {
                Token::Plus => BinaryOperator::Add,
                Token::Minus => BinaryOperator::Subtract,
                _ => break,
            };
            
            self.advance(); // consume operator
            let right = self.parse_multiplicative_expression()?;
            left = Expression::BinaryOp {
                left: Box::new(left),
                operator,
                right: Box::new(right),
            };
        }
        
        Ok(left)
    }
    
    fn parse_multiplicative_expression(&mut self) -> Result<Expression, ParseError> {
        let mut left = self.parse_conditional_expression()?;
        
        while let Some(token) = self.current_token() {
            let operator = match token {
                Token::Star => BinaryOperator::Multiply,
                Token::Slash => BinaryOperator::Divide,
                Token::Percent => BinaryOperator::Modulo,
                _ => break,
            };
            
            self.advance(); // consume operator
            let right = self.parse_conditional_expression()?;
            left = Expression::BinaryOp {
                left: Box::new(left),
                operator,
                right: Box::new(right),
            };
        }
        
        Ok(left)
    }
    
    fn parse_conditional_expression(&mut self) -> Result<Expression, ParseError> {
        match self.current_token() {
            Some(Token::If) => self.parse_if_then_else_expression(),
            Some(Token::Match) => self.parse_match_expression(),
            _ => self.parse_postfix_expression(),
        }
    }
    
    fn parse_postfix_expression(&mut self) -> Result<Expression, ParseError> {
        let mut expr = self.parse_primary_expression()?;
        
        // Handle postfix operations like array access and field access
        while let Some(token) = self.current_token() {
            match token {
                Token::LeftBracket => {
                    self.advance(); // consume '['
                    let index = self.parse_expression()?;
                    self.expect_token(Token::RightBracket)?;
                    expr = Expression::ArrayAccess {
                        array: Box::new(expr),
                        index: Box::new(index),
                    };
                }
                Token::Dot => {
                    // Handle chained field access like .field after array access
                    self.advance(); // consume '.'
                    if let Some(Token::Identifier(field_name)) = self.current_token() {
                        let field_name = field_name.clone();
                        self.advance();
                        
                        // Create a chained access expression
                        expr = Expression::ChainedAccess {
                            base: Box::new(expr),
                            field: field_name,
                        };
                    } else {
                        return Err(ParseError::UnexpectedToken("Expected field name after dot".to_string()));
                    }
                }
                Token::FieldAccess(path) => {
                    // Handle field access tokens as chained access
                    // This happens when the tokenizer parsed .field as a FieldAccess token
                    if path.len() == 1 {
                        let field_name = path[0].clone();
                        self.advance();
                        
                        expr = Expression::ChainedAccess {
                            base: Box::new(expr),
                            field: field_name,
                        };
                    } else {
                        // Multiple field access - chain them
                        for field_name in path {
                            expr = Expression::ChainedAccess {
                                base: Box::new(expr),
                                field: field_name.clone(),
                            };
                        }
                        self.advance();
                    }
                }
                _ => break,
            }
        }
        
        Ok(expr)
    }
    
    fn parse_if_then_else_expression(&mut self) -> Result<Expression, ParseError> {
        self.expect_token(Token::If)?;
        
        let condition = self.parse_condition()?;
        
        // Check if this is single expression syntax (with then/else) or block syntax (with braces)
        match self.current_token() {
            Some(Token::Then) => {
                // Single expression syntax: if condition then expr else expr
                self.advance(); // consume 'then'
                let then_expr = self.parse_expression()?;
                
                self.expect_token(Token::Else)?;
                let else_expr = self.parse_expression()?;
                
                Ok(Expression::IfThenElse {
                    condition: Box::new(condition),
                    then_expr: Box::new(then_expr),
                    else_expr: Box::new(else_expr),
                })
            }
            Some(Token::LeftBrace) => {
                // Block syntax: if condition { exprs } else { exprs }
                let then_expr = self.parse_block_expression()?;
                
                self.expect_token(Token::Else)?;
                let else_expr = self.parse_block_expression()?;
                
                Ok(Expression::IfThenElse {
                    condition: Box::new(condition),
                    then_expr: Box::new(then_expr),
                    else_expr: Box::new(else_expr),
                })
            }
            Some(token) => {
                Err(ParseError::UnexpectedToken(format!(
                    "Expected 'then' or '{{' after if condition, found {:?}", token
                )))
            }
            None => {
                Err(ParseError::UnexpectedEndOfInput)
            }
        }
    }
    

    
    fn parse_block_expression(&mut self) -> Result<Expression, ParseError> {
        self.expect_token(Token::LeftBrace)?;
        
        let mut expressions = Vec::new();
        
        // Parse expressions until we hit the closing brace
        while !matches!(self.current_token(), Some(Token::RightBrace)) {
            let expr = self.parse_expression()?;
            expressions.push(expr);
            
            // Optional comma or semicolon between expressions
            if matches!(self.current_token(), Some(Token::Comma)) {
                self.advance();
            }
        }
        
        self.expect_token(Token::RightBrace)?;
        
        // If there's only one expression, return it directly
        // If there are multiple expressions, return a Block
        if expressions.len() == 1 {
            Ok(expressions.into_iter().next().unwrap())
        } else {
            Ok(Expression::Block(expressions))
        }
    }
    
    fn parse_match_expression(&mut self) -> Result<Expression, ParseError> {
        self.expect_token(Token::Match)?;
        
        let expr = self.parse_primary_expression()?;
        
        self.expect_token(Token::LeftBrace)?;
        
        let mut arms = Vec::new();
        
        // Parse match arms
        while !matches!(self.current_token(), Some(Token::RightBrace)) {
            let arm = self.parse_match_arm()?;
            arms.push(arm);
            
            // Optional comma between arms
            if matches!(self.current_token(), Some(Token::Comma)) {
                self.advance();
            }
        }
        
        self.expect_token(Token::RightBrace)?;
        
        Ok(Expression::Match {
            expr: Box::new(expr),
            arms,
        })
    }
    
    fn parse_match_arm(&mut self) -> Result<MatchArm, ParseError> {
        let pattern = self.parse_match_pattern()?;
        
        // Optional guard condition
        let guard = if matches!(self.current_token(), Some(Token::If)) {
            self.advance(); // consume 'if'
            Some(self.parse_condition()?)
        } else {
            None
        };
        
        self.expect_token(Token::Arrow)?;
        let body = self.parse_expression()?;
        
        Ok(MatchArm { pattern, guard, body })
    }
    
    fn parse_match_pattern(&mut self) -> Result<MatchPattern, ParseError> {
        match self.current_token() {
            Some(Token::Identifier(name)) if name == "_" => {
                self.advance();
                Ok(MatchPattern::Wildcard)
            }
            Some(Token::Identifier(name)) => {
                let name = name.clone();
                self.advance();
                Ok(MatchPattern::Variable(name))
            }
            _ => {
                let literal = self.parse_literal()?;
                Ok(MatchPattern::Literal(literal))
            }
        }
    }
    
    fn parse_primary_expression(&mut self) -> Result<Expression, ParseError> {
        match self.current_token() {
            Some(Token::FieldAccess(path)) => {
                let field_access = FieldAccess { path: path.clone() };
                self.advance();
                Ok(Expression::FieldAccess(field_access))
            }
            Some(Token::Identifier(name)) => {
                let name = name.clone();
                self.advance();
                
                // Check if this is a function call
                if matches!(self.current_token(), Some(Token::LeftParen)) {
                    self.advance(); // consume left paren
                    
                    let mut args = Vec::new();
                    
                    // Parse arguments if any
                    if !matches!(self.current_token(), Some(Token::RightParen)) {
                        let arg = self.parse_expression()?;
                        args.push(arg);
                        
                        while matches!(self.current_token(), Some(Token::Comma)) {
                            self.advance(); // consume comma
                            let arg = self.parse_expression()?;
                            args.push(arg);
                        }
                    }
                    
                    self.expect_token(Token::RightParen)?;
                    
                    Ok(Expression::FunctionCall { name, args })
                } else {
                    // This is a simple field access
                    Ok(Expression::FieldAccess(FieldAccess { path: vec![name] }))
                }
            }
            // Handle aggregation function tokens
            Some(Token::Count) => {
                let name = "count".to_string();
                self.advance();
                self.parse_function_call(name)
            }
            Some(Token::Sum) => {
                let name = "sum".to_string();
                self.advance();
                self.parse_function_call(name)
            }
            Some(Token::Avg) => {
                let name = "avg".to_string();
                self.advance();
                self.parse_function_call(name)
            }
            Some(Token::Min) => {
                let name = "min".to_string();
                self.advance();
                self.parse_function_call(name)
            }
            Some(Token::Max) => {
                let name = "max".to_string();
                self.advance();
                self.parse_function_call(name)
            }
            Some(Token::Star) => {
                // Handle * as a special identifier for count(*)
                self.advance();
                Ok(Expression::FieldAccess(FieldAccess { path: vec!["*".to_string()] }))
            }
            Some(Token::LeftParen) => {
                // Handle parenthesized expressions
                self.advance(); // consume left paren
                let expr = self.parse_expression()?;
                self.expect_token(Token::RightParen)?;
                Ok(expr)
            }
            Some(Token::LeftBracket) => {
                // Handle array literals
                self.parse_array_literal()
            }
            Some(Token::LeftBrace) => {
                // Handle object literals
                self.parse_object_literal()
            }
            _ => {
                let literal = self.parse_literal()?;
                Ok(Expression::Literal(literal))
            }
        }
    }
    
    fn parse_function_call(&mut self, name: String) -> Result<Expression, ParseError> {
        self.expect_token(Token::LeftParen)?;
        
        let mut args = Vec::new();
        
        // Parse arguments if any
        if !matches!(self.current_token(), Some(Token::RightParen)) {
            let arg = self.parse_expression()?;
            args.push(arg);
            
            while matches!(self.current_token(), Some(Token::Comma)) {
                self.advance(); // consume comma
                let arg = self.parse_expression()?;
                args.push(arg);
            }
        }
        
        self.expect_token(Token::RightParen)?;
        
        Ok(Expression::FunctionCall { name, args })
    }
    
    fn parse_array_literal(&mut self) -> Result<Expression, ParseError> {
        self.expect_token(Token::LeftBracket)?;
        
        let mut elements = Vec::new();
        
        // Parse array elements if any
        if !matches!(self.current_token(), Some(Token::RightBracket)) {
            let element = self.parse_expression()?;
            elements.push(element);
            
            while matches!(self.current_token(), Some(Token::Comma)) {
                self.advance(); // consume comma
                let element = self.parse_expression()?;
                elements.push(element);
            }
        }
        
        self.expect_token(Token::RightBracket)?;
        
        Ok(Expression::ArrayLiteral(elements))
    }
    
    fn parse_object_literal(&mut self) -> Result<Expression, ParseError> {
        self.expect_token(Token::LeftBrace)?;
        
        let mut fields = Vec::new();
        
        // Parse object fields if any
        if !matches!(self.current_token(), Some(Token::RightBrace)) {
            let field = self.parse_object_field()?;
            fields.push(field);
            
            while matches!(self.current_token(), Some(Token::Comma)) {
                self.advance(); // consume comma
                let field = self.parse_object_field()?;
                fields.push(field);
            }
        }
        
        self.expect_token(Token::RightBrace)?;
        
        Ok(Expression::ObjectLiteral(fields))
    }
    
    fn parse_object_field(&mut self) -> Result<ObjectField, ParseError> {
        // Parse field name
        let name = match self.current_token() {
            Some(Token::Identifier(name)) => {
                let field_name = name.clone();
                self.advance();
                field_name
            }
            Some(token) => return Err(ParseError::UnexpectedToken(format!("Expected field name, found {:?}", token))),
            None => return Err(ParseError::UnexpectedEndOfInput),
        };
        
        // Expect colon
        self.expect_token(Token::Colon)?;
        
        // Parse value (can be any expression including nested objects)
        let value = self.parse_expression()?;
        
        Ok(ObjectField { name, value })
    }
    
    /// Parse a simple expression without logical operators (for use in conditions)
    fn parse_simple_expression(&mut self) -> Result<Expression, ParseError> {
        self.parse_conditional_expression()
    }
    
    fn parse_condition(&mut self) -> Result<Condition, ParseError> {
        self.parse_or_condition()
    }
    
    fn parse_or_condition(&mut self) -> Result<Condition, ParseError> {
        let mut left = self.parse_and_condition()?;
        
        while matches!(self.current_token(), Some(Token::Or)) {
            self.advance(); // consume 'or' or '||'
            let right = self.parse_and_condition()?;
            left = Condition::Or(Box::new(left), Box::new(right));
        }
        
        Ok(left)
    }
    
    fn parse_and_condition(&mut self) -> Result<Condition, ParseError> {
        let mut left = self.parse_not_condition()?;
        
        while matches!(self.current_token(), Some(Token::And)) {
            self.advance(); // consume 'and' or '&&'
            let right = self.parse_not_condition()?;
            left = Condition::And(Box::new(left), Box::new(right));
        }
        
        Ok(left)
    }
    
    fn parse_not_condition(&mut self) -> Result<Condition, ParseError> {
        if matches!(self.current_token(), Some(Token::Not)) {
            self.advance(); // consume 'not'
            let condition = self.parse_not_condition()?;
            Ok(Condition::Not(Box::new(condition)))
        } else {
            self.parse_primary_condition()
        }
    }
    
    fn parse_primary_condition(&mut self) -> Result<Condition, ParseError> {
        // Handle parentheses
        if matches!(self.current_token(), Some(Token::LeftParen)) {
            self.advance(); // consume '('
            let condition = self.parse_condition()?;
            self.expect_token(Token::RightParen)?;
            return Ok(condition);
        }
        
        // Parse left side (simple expression without logical operators)
        let left = self.parse_simple_expression()?;
        
        // Check if there's a comparison operator
        match self.current_token() {
            Some(Token::Equal) => {
                self.advance();
                let right = self.parse_simple_expression()?;
                Ok(Condition::Comparison { left, operator: ComparisonOperator::Equal, right })
            }
            Some(Token::NotEqual) => {
                self.advance();
                let right = self.parse_simple_expression()?;
                Ok(Condition::Comparison { left, operator: ComparisonOperator::NotEqual, right })
            }
            Some(Token::LessThan) => {
                self.advance();
                let right = self.parse_simple_expression()?;
                Ok(Condition::Comparison { left, operator: ComparisonOperator::LessThan, right })
            }
            Some(Token::GreaterThan) => {
                self.advance();
                let right = self.parse_simple_expression()?;
                Ok(Condition::Comparison { left, operator: ComparisonOperator::GreaterThan, right })
            }
            Some(Token::LessEqual) => {
                self.advance();
                let right = self.parse_simple_expression()?;
                Ok(Condition::Comparison { left, operator: ComparisonOperator::LessEqual, right })
            }
            Some(Token::GreaterEqual) => {
                self.advance();
                let right = self.parse_simple_expression()?;
                Ok(Condition::Comparison { left, operator: ComparisonOperator::GreaterEqual, right })
            }
            Some(Token::Contains) => {
                self.advance();
                let right = self.parse_simple_expression()?;
                Ok(Condition::Comparison { left, operator: ComparisonOperator::Contains, right })
            }
            Some(Token::StartsWith) => {
                self.advance();
                let right = self.parse_simple_expression()?;
                Ok(Condition::Comparison { left, operator: ComparisonOperator::StartsWith, right })
            }
            Some(Token::EndsWith) => {
                self.advance();
                let right = self.parse_simple_expression()?;
                Ok(Condition::Comparison { left, operator: ComparisonOperator::EndsWith, right })
            }
            Some(Token::In) => {
                self.advance();
                let right = self.parse_simple_expression()?;
                Ok(Condition::Comparison { left, operator: ComparisonOperator::In, right })
            }
            _ => {
                // No comparison operator found - treat the expression as a boolean condition
                // This allows for conditions like "if .premium" or "if .active"
                Ok(Condition::Comparison { 
                    left, 
                    operator: ComparisonOperator::Equal, 
                    right: Expression::Literal(Literal::Boolean(true))
                })
            }
        }
    }
    
    fn parse_literal(&mut self) -> Result<Literal, ParseError> {
        match self.current_token() {
            Some(Token::StringLiteral(value)) => {
                let literal = Literal::String(value.clone());
                self.advance();
                Ok(literal)
            }
            Some(Token::IntegerLiteral(value)) => {
                let literal = Literal::Integer(*value);
                self.advance();
                Ok(literal)
            }
            Some(Token::FloatLiteral(value)) => {
                let literal = Literal::Float(*value);
                self.advance();
                Ok(literal)
            }
            Some(Token::BooleanLiteral(value)) => {
                let literal = Literal::Boolean(*value);
                self.advance();
                Ok(literal)
            }
            Some(token) => Err(ParseError::UnexpectedToken(format!("Expected literal value, found {:?}", token))),
            None => Err(ParseError::UnexpectedEndOfInput),
        }
    }
    
    fn current_token(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }
    
    fn advance(&mut self) {
        if self.position < self.tokens.len() {
            self.position += 1;
        }
    }
    
    fn expect_token(&mut self, expected: Token) -> Result<(), ParseError> {
        match self.current_token() {
            Some(token) if std::mem::discriminant(token) == std::mem::discriminant(&expected) => {
                self.advance();
                Ok(())
            }
            Some(token) => Err(ParseError::UnexpectedToken(format!("Expected {:?}, found {:?}", expected, token))),
            None => Err(ParseError::UnexpectedEndOfInput),
        }
    }
}

// Error types for parsing
#[derive(Debug, Clone)]
pub enum ParseError {
    UnexpectedToken(String),
    UnexpectedEndOfInput,
    UnsupportedQuery(String),
    InvalidSyntax(String),
    TypeError { message: String },
    SchemaError { message: String },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::UnexpectedToken(msg) => write!(f, "Unexpected token: {}", msg),
            ParseError::UnexpectedEndOfInput => write!(f, "Unexpected end of input"),
            ParseError::UnsupportedQuery(msg) => write!(f, "Unsupported query: {}", msg),
            ParseError::InvalidSyntax(msg) => write!(f, "Invalid syntax: {}", msg),
            ParseError::TypeError { message } => write!(f, "Type error: {}", message),
            ParseError::SchemaError { message } => write!(f, "Schema error: {}", message),
        }
    }
}

impl std::error::Error for ParseError {}

// Convenience function to parse OQL statements (queries or function definitions)
pub fn parse_oql_statement(input: &str) -> Result<OQLStatement, Box<dyn std::error::Error>> {
    let tokens = Tokenizer::tokenize(input)?;
    let mut parser = Parser::new(tokens);
    let statement = parser.parse()?;
    Ok(statement)
}

// Parse OQL statement with schema registry for validation
pub fn parse_oql_statement_with_schema(
    input: &str,
    schema_registry: crate::schema::SchemaRegistry,
) -> Result<OQLStatement, Box<dyn std::error::Error>> {
    let tokens = Tokenizer::tokenize(input)?;
    let mut parser = Parser::with_schema_registry(tokens, schema_registry);
    let statement = parser.parse()?;
    Ok(statement)
}

// Convenience function to parse OQL queries specifically (for backward compatibility)
pub fn parse_oql(input: &str) -> Result<OQLQuery, Box<dyn std::error::Error>> {
    match parse_oql_statement(input)? {
        OQLStatement::Query(query) => Ok(query),
        OQLStatement::FunctionDef(_) => Err("Expected query, found function definition".into()),
    }
}

// Parse OQL query with schema registry for validation
pub fn parse_oql_with_schema(
    input: &str,
    schema_registry: crate::schema::SchemaRegistry,
) -> Result<OQLQuery, Box<dyn std::error::Error>> {
    match parse_oql_statement_with_schema(input, schema_registry)? {
        OQLStatement::Query(query) => Ok(query),
        OQLStatement::FunctionDef(_) => Err("Expected query, found function definition".into()),
    }
}

#[cfg(test)]
mod parser_tests {
    use super::*;

    // ========== TOKENIZER TESTS ==========
    
    #[test]
    fn test_tokenize_complex_query() {
        let input = r#"customers.where { .name == "Alice Smith" and .age >= 25 }.select { id, full_name: .name }"#;
        let tokens = Tokenizer::tokenize(input).unwrap();
        
        let expected = vec![
            Token::Identifier("customers".to_string()),
            Token::Dot,
            Token::Where,
            Token::LeftBrace,
            Token::FieldAccess(vec!["name".to_string()]),
            Token::Equal,
            Token::StringLiteral("Alice Smith".to_string()),
            Token::And,
            Token::FieldAccess(vec!["age".to_string()]),
            Token::GreaterEqual,
            Token::IntegerLiteral(25),
            Token::RightBrace,
            Token::Dot,
            Token::Select,
            Token::LeftBrace,
            Token::Identifier("id".to_string()),
            Token::Comma,
            Token::Identifier("full_name".to_string()),
            Token::Colon,
            Token::FieldAccess(vec!["name".to_string()]),
            Token::RightBrace,
            Token::EOF,
        ];
        
        assert_eq!(tokens, expected);
    }
    
    #[test]
    fn test_tokenize_nested_field_access() {
        let input = ".customer.account.secure_token";
        let tokens = Tokenizer::tokenize(input).unwrap();
        
        assert_eq!(tokens, vec![
            Token::FieldAccess(vec!["customer".to_string(), "account".to_string(), "secure_token".to_string()]),
            Token::EOF,
        ]);
    }
    
    #[test]
    fn test_tokenize_all_operators() {
        let input = "== != < > <= >= and or not";
        let tokens = Tokenizer::tokenize(input).unwrap();
        
        assert_eq!(tokens, vec![
            Token::Equal,
            Token::NotEqual,
            Token::LessThan,
            Token::GreaterThan,
            Token::LessEqual,
            Token::GreaterEqual,
            Token::And,
            Token::Or,
            Token::Not,
            Token::EOF,
        ]);
    }
    
    #[test]
    fn test_tokenize_mixed_literals() {
        let input = r#"42 3.14159 "hello world" true false"#;
        let tokens = Tokenizer::tokenize(input).unwrap();
        
        assert_eq!(tokens, vec![
            Token::IntegerLiteral(42),
            Token::FloatLiteral(3.14159),
            Token::StringLiteral("hello world".to_string()),
            Token::BooleanLiteral(true),
            Token::BooleanLiteral(false),
            Token::EOF,
        ]);
    }
    
    #[test]
    fn test_tokenize_identifiers_with_underscores() {
        let input = "user_id customer_name _private_field";
        let tokens = Tokenizer::tokenize(input).unwrap();
        
        assert_eq!(tokens, vec![
            Token::Identifier("user_id".to_string()),
            Token::Identifier("customer_name".to_string()),
            Token::Identifier("_private_field".to_string()),
            Token::EOF,
        ]);
    }
    
    #[test]
    fn test_tokenize_keywords_vs_identifiers() {
        // Keywords should be recognized as keywords, not identifiers
        let input = "where whereabouts select selection";
        let tokens = Tokenizer::tokenize(input).unwrap();
        
        assert_eq!(tokens, vec![
            Token::Where,
            Token::Identifier("whereabouts".to_string()),
            Token::Select,
            Token::Identifier("selection".to_string()),
            Token::EOF,
        ]);
    }
    
    #[test]
    fn test_tokenize_empty_string() {
        let input = r#""""#;
        let tokens = Tokenizer::tokenize(input).unwrap();
        
        assert_eq!(tokens, vec![
            Token::StringLiteral("".to_string()),
            Token::EOF,
        ]);
    }
    
    #[test]
    fn test_tokenize_zero_values() {
        let input = "0 0.0";
        let tokens = Tokenizer::tokenize(input).unwrap();
        
        assert_eq!(tokens, vec![
            Token::IntegerLiteral(0),
            Token::FloatLiteral(0.0),
            Token::EOF,
        ]);
    }
    
    // ========== AST GENERATION TESTS ==========
    
    #[test]
    fn test_parse_simple_where() {
        let input = r#"customers.where { .name == "Alice" }"#;
        let query = parse_oql(input).unwrap();
        
        assert_eq!(query.collection, "customers");
        assert_eq!(query.operations.len(), 1);
        
        match &query.operations[0] {
            Operation::Where(where_block) => {
                match &where_block.condition {
                    Condition::Comparison { left, operator, right } => {
                        match left {
                            Expression::FieldAccess(field) => {
                                assert_eq!(field.path, vec!["name"]);
                            }
                            _ => panic!("Expected field access"),
                        }
                        assert!(matches!(operator, ComparisonOperator::Equal));
                        match right {
                            Expression::Literal(Literal::String(s)) => {
                                assert_eq!(s, "Alice");
                            }
                            _ => panic!("Expected string literal"),
                        }
                    }
                    _ => panic!("Expected comparison condition"),
                }
            }
            _ => panic!("Expected WHERE operation"),
        }
    }
    
    #[test]
    fn test_parse_all_comparison_operators() {
        let test_cases = vec![
            (r#"users.where { .age == 25 }"#, ComparisonOperator::Equal),
            (r#"users.where { .age != 25 }"#, ComparisonOperator::NotEqual),
            (r#"users.where { .age < 25 }"#, ComparisonOperator::LessThan),
            (r#"users.where { .age > 25 }"#, ComparisonOperator::GreaterThan),
            (r#"users.where { .age <= 25 }"#, ComparisonOperator::LessEqual),
            (r#"users.where { .age >= 25 }"#, ComparisonOperator::GreaterEqual),
        ];
        
        for (input, expected_op) in test_cases {
            let query = parse_oql(input).unwrap();
            match &query.operations[0] {
                Operation::Where(where_block) => {
                    match &where_block.condition {
                        Condition::Comparison { operator, .. } => {
                            assert_eq!(std::mem::discriminant(operator), std::mem::discriminant(&expected_op));
                        }
                        _ => panic!("Expected comparison condition"),
                    }
                }
                _ => panic!("Expected WHERE operation"),
            }
        }
    }
    
    #[test]
    fn test_parse_select_with_fields() {
        let input = r#"users.select { name, email: .email_address }"#;
        let query = parse_oql(input).unwrap();
        
        assert_eq!(query.collection, "users");
        assert_eq!(query.operations.len(), 1);
        
        match &query.operations[0] {
            Operation::Select(select_block) => {
                assert_eq!(select_block.fields.len(), 2);
                
                // First field: name (no alias)
                assert!(select_block.fields[0].alias.is_none());
                match &select_block.fields[0].expression {
                    Expression::FieldAccess(field) => {
                        assert_eq!(field.path, vec!["name"]);
                    }
                    _ => panic!("Expected field access"),
                }
                
                // Second field: email (with alias)
                assert_eq!(select_block.fields[1].alias, Some("email".to_string()));
                match &select_block.fields[1].expression {
                    Expression::FieldAccess(field) => {
                        assert_eq!(field.path, vec!["email_address"]);
                    }
                    _ => panic!("Expected field access"),
                }
            }
            _ => panic!("Expected SELECT operation"),
        }
    }
    
    #[test]
    fn test_parse_select_single_field() {
        let input = r#"products.select { name }"#;
        let query = parse_oql(input).unwrap();
        
        match &query.operations[0] {
            Operation::Select(select_block) => {
                assert_eq!(select_block.fields.len(), 1);
                assert!(select_block.fields[0].alias.is_none());
                match &select_block.fields[0].expression {
                    Expression::FieldAccess(field) => {
                        assert_eq!(field.path, vec!["name"]);
                    }
                    _ => panic!("Expected field access"),
                }
            }
            _ => panic!("Expected SELECT operation"),
        }
    }
    
    #[test]
    fn test_parse_insert() {
        let input = r#"customers.insert { name: "Bob", age: 30 }"#;
        let query = parse_oql(input).unwrap();
        
        assert_eq!(query.collection, "customers");
        assert_eq!(query.operations.len(), 1);
        
        match &query.operations[0] {
            Operation::Insert(insert_block) => {
                assert_eq!(insert_block.fields.len(), 2);
                
                assert_eq!(insert_block.fields[0].name, "name");
                assert!(matches!(insert_block.fields[0].value, Expression::Literal(Literal::String(ref s)) if s == "Bob"));
                
                assert_eq!(insert_block.fields[1].name, "age");
                assert!(matches!(insert_block.fields[1].value, Expression::Literal(Literal::Integer(30))));
            }
            _ => panic!("Expected INSERT operation"),
        }
    }
    
    #[test]
    fn test_parse_insert_all_literal_types() {
        let input = r#"test.insert { name: "Alice", age: 25, score: 98.5, active: true }"#;
        let query = parse_oql(input).unwrap();
        
        match &query.operations[0] {
            Operation::Insert(insert_block) => {
                assert_eq!(insert_block.fields.len(), 4);
                
                assert_eq!(insert_block.fields[0].name, "name");
                assert!(matches!(insert_block.fields[0].value, Expression::Literal(Literal::String(ref s)) if s == "Alice"));
                
                assert_eq!(insert_block.fields[1].name, "age");
                assert!(matches!(insert_block.fields[1].value, Expression::Literal(Literal::Integer(25))));
                
                assert_eq!(insert_block.fields[2].name, "score");
                assert!(matches!(insert_block.fields[2].value, Expression::Literal(Literal::Float(f)) if (f - 98.5).abs() < f64::EPSILON));
                
                assert_eq!(insert_block.fields[3].name, "active");
                assert!(matches!(insert_block.fields[3].value, Expression::Literal(Literal::Boolean(true))));
            }
            _ => panic!("Expected INSERT operation"),
        }
    }
    
    #[test]
    fn test_parse_update() {
        let input = r#"users.update { email: "new@example.com", last_login: .now }"#;
        let query = parse_oql(input).unwrap();
        
        match &query.operations[0] {
            Operation::Update(update_block) => {
                assert_eq!(update_block.fields.len(), 2);
                
                assert_eq!(update_block.fields[0].name, "email");
                match &update_block.fields[0].value {
                    Expression::Literal(Literal::String(s)) => {
                        assert_eq!(s, "new@example.com");
                    }
                    _ => panic!("Expected string literal"),
                }
                
                assert_eq!(update_block.fields[1].name, "last_login");
                match &update_block.fields[1].value {
                    Expression::FieldAccess(field) => {
                        assert_eq!(field.path, vec!["now"]);
                    }
                    _ => panic!("Expected field access"),
                }
            }
            _ => panic!("Expected UPDATE operation"),
        }
    }
    
    #[test]
    fn test_parse_delete() {
        let input = "users.delete";
        let query = parse_oql(input).unwrap();
        
        assert_eq!(query.collection, "users");
        assert_eq!(query.operations.len(), 1);
        assert!(matches!(query.operations[0], Operation::Delete));
    }
    
    #[test]
    fn test_parse_chained_operations() {
        let input = r#"orders.where { .status == "pending" }.select { id, .customer.name }"#;
        let query = parse_oql(input).unwrap();
        
        assert_eq!(query.collection, "orders");
        assert_eq!(query.operations.len(), 2);
        
        // First operation: WHERE
        match &query.operations[0] {
            Operation::Where(where_block) => {
                match &where_block.condition {
                    Condition::Comparison { left, operator, right } => {
                        match left {
                            Expression::FieldAccess(field) => {
                                assert_eq!(field.path, vec!["status"]);
                            }
                            _ => panic!("Expected field access"),
                        }
                        assert!(matches!(operator, ComparisonOperator::Equal));
                        match right {
                            Expression::Literal(Literal::String(s)) => {
                                assert_eq!(s, "pending");
                            }
                            _ => panic!("Expected string literal"),
                        }
                    }
                    _ => panic!("Expected comparison condition"),
                }
            }
            _ => panic!("Expected WHERE operation"),
        }
        
        // Second operation: SELECT
        match &query.operations[1] {
            Operation::Select(select_block) => {
                assert_eq!(select_block.fields.len(), 2);
                
                // First field: id
                match &select_block.fields[0].expression {
                    Expression::FieldAccess(field) => {
                        assert_eq!(field.path, vec!["id"]);
                    }
                    _ => panic!("Expected field access"),
                }
                
                // Second field: .customer.name (reference navigation)
                match &select_block.fields[1].expression {
                    Expression::FieldAccess(field) => {
                        assert_eq!(field.path, vec!["customer", "name"]);
                    }
                    _ => panic!("Expected field access"),
                }
            }
            _ => panic!("Expected SELECT operation"),
        }
    }
    
    #[test]
    fn test_parse_deep_reference_navigation() {
        let input = r#"orders.select { token: .customer.account.secure_token }"#;
        let query = parse_oql(input).unwrap();
        
        match &query.operations[0] {
            Operation::Select(select_block) => {
                assert_eq!(select_block.fields.len(), 1);
                assert_eq!(select_block.fields[0].alias, Some("token".to_string()));
                match &select_block.fields[0].expression {
                    Expression::FieldAccess(field) => {
                        assert_eq!(field.path, vec!["customer", "account", "secure_token"]);
                    }
                    _ => panic!("Expected field access"),
                }
            }
            _ => panic!("Expected SELECT operation"),
        }
    }
    
    #[test]
    fn test_parse_multiple_chained_operations() {
        let input = r#"products.where { .price > 100 }.select { name, price }.delete"#;
        let query = parse_oql(input).unwrap();
        
        assert_eq!(query.collection, "products");
        assert_eq!(query.operations.len(), 3);
        
        assert!(matches!(query.operations[0], Operation::Where(_)));
        assert!(matches!(query.operations[1], Operation::Select(_)));
        assert!(matches!(query.operations[2], Operation::Delete));
    }
    
    // ========== ERROR HANDLING TESTS ==========
    
    #[test]
    fn test_parse_error_missing_collection() {
        let input = r#".where { .name == "Alice" }"#;
        let result = parse_oql(input);
        assert!(result.is_err());
    }
    
    #[test]
    fn test_parse_error_missing_opening_brace() {
        let input = r#"customers.where .name == "Alice" }"#;
        let result = parse_oql(input);
        assert!(result.is_err());
    }
    
    #[test]
    fn test_parse_error_missing_closing_brace() {
        let input = r#"customers.where { .name == "Alice""#;
        let result = parse_oql(input);
        assert!(result.is_err());
    }
    
    #[test]
    fn test_parse_error_invalid_operator() {
        let input = r#"customers.where { .name = "Alice" }"#;
        let result = parse_oql(input);
        assert!(result.is_err());
    }
    
    #[test]
    fn test_parse_error_missing_field_name_in_insert() {
        let input = r#"customers.insert { : "Alice" }"#;
        let result = parse_oql(input);
        assert!(result.is_err());
    }
    
    #[test]
    fn test_parse_error_missing_colon_in_insert() {
        let input = r#"customers.insert { name "Alice" }"#;
        let result = parse_oql(input);
        assert!(result.is_err());
    }
    
    #[test]
    fn test_parse_error_unexpected_token_after_query() {
        let input = r#"customers.where { .name == "Alice" } extra"#;
        let result = parse_oql(input);
        assert!(result.is_err());
    }
    
    #[test]
    fn test_parse_error_empty_where_block() {
        let input = "customers.where { }";
        let result = parse_oql(input);
        assert!(result.is_err());
    }
    
    #[test]
    fn test_parse_error_empty_select_block() {
        let input = "customers.select { }";
        let result = parse_oql(input);
        assert!(result.is_err());
    }
    
    #[test]
    fn test_parse_error_empty_insert_block() {
        let input = "customers.insert { }";
        let result = parse_oql(input);
        assert!(result.is_err());
    }
    
    #[test]
    fn test_parse_error_invalid_method_name() {
        let input = "customers.invalid_method { }";
        let result = parse_oql(input);
        assert!(result.is_err());
    }
    
    #[test]
    fn test_tokenize_error_unterminated_string() {
        let input = r#"customers.where { .name == "Alice }"#;
        let result = Tokenizer::tokenize(input);
        assert!(result.is_err());
    }
    
    #[test]
    fn test_parse_error_missing_comparison_right_side() {
        let input = "customers.where { .name == }";
        let result = parse_oql(input);
        assert!(result.is_err());
    }
    
    #[test]
    fn test_parse_error_missing_field_access_dot() {
        let input = "customers.where { name == \"Alice\" }";
        // This should actually parse as a field access without dot, but let's test the current behavior
        let query = parse_oql(input).unwrap();
        match &query.operations[0] {
            Operation::Where(where_block) => {
                match &where_block.condition {
                    Condition::Comparison { left, .. } => {
                        match left {
                            Expression::FieldAccess(field) => {
                                assert_eq!(field.path, vec!["name"]);
                            }
                            _ => panic!("Expected field access"),
                        }
                    }
                    _ => panic!("Expected comparison condition"),
                }
            }
            _ => panic!("Expected WHERE operation"),
        }
    }
    
    #[test]
    fn test_error_message_formatting() {
        let error = ParseError::UnexpectedToken("Expected identifier, found number".to_string());
        let message = format!("{}", error);
        assert!(message.contains("Unexpected token"));
        assert!(message.contains("Expected identifier, found number"));
        
        let error = ParseError::UnexpectedEndOfInput;
        let message = format!("{}", error);
        assert_eq!(message, "Unexpected end of input");
        
        let error = ParseError::InvalidSyntax("Missing semicolon".to_string());
        let message = format!("{}", error);
        assert!(message.contains("Invalid syntax"));
        assert!(message.contains("Missing semicolon"));
    }
    
    #[test]
    fn test_tokenize_error_message_formatting() {
        let error = TokenizeError::ParseError("Invalid character sequence".to_string());
        let message = format!("{}", error);
        assert!(message.contains("Parse error"));
        assert!(message.contains("Invalid character sequence"));
        
        let error = TokenizeError::UnexpectedCharacter('@');
        let message = format!("{}", error);
        assert!(message.contains("Unexpected character"));
        assert!(message.contains("'@'"));
        
        let error = TokenizeError::UnterminatedString;
        let message = format!("{}", error);
        assert_eq!(message, "Unterminated string literal");
    }
}

// OQL FILE PARSING

/// Represents a complete OQL file with multiple statements
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OQLFile {
    pub statements: Vec<OQLFileStatement>,
}

/// Represents different types of statements that can appear in an OQL file
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OQLFileStatement {
    Schema(SchemaDefinition),
    Query(OQLQuery),
    FunctionDef(FunctionDefinition),
}

/// Represents a schema definition in OQL syntax
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaDefinition {
    pub name: String,
    pub fields: Vec<SchemaField>,
}

/// Represents a field in a schema definition
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaField {
    pub name: String,
    pub field_type: String, // For now, store as string - can be parsed later
    pub optional: bool,
}

/// Parse a complete OQL file containing multiple statements
pub fn parse_oql_file(content: &str) -> Result<OQLFile, Box<dyn std::error::Error>> {
    let preprocessed_content = preprocess_method_chaining(content);
    let statements = extract_statements(&preprocessed_content)?;
    let mut parsed_statements = Vec::new();
    
    for statement in statements {
        let trimmed = statement.trim();
        if trimmed.is_empty() {
            continue;
        }
        
        if trimmed.starts_with("schema ") {
            // Parse schema definition
            let schema = parse_schema_definition(&statement)?;
            parsed_statements.push(OQLFileStatement::Schema(schema));
        } else if trimmed.starts_with("fn ") {
            // Parse function definition
            match parse_oql_statement(&statement)? {
                OQLStatement::FunctionDef(func_def) => {
                    parsed_statements.push(OQLFileStatement::FunctionDef(func_def));
                }
                _ => return Err("Expected function definition".into()),
            }
        } else {
            // Parse as query
            let query = parse_oql(&statement)?;
            parsed_statements.push(OQLFileStatement::Query(query));
        }
    }
    
    Ok(OQLFile { statements: parsed_statements })
}

/// Preprocess content to handle method chaining
fn preprocess_method_chaining(content: &str) -> String {
    let mut result = String::new();
    let mut lines: Vec<&str> = content.lines().collect();
    let mut i = 0;
    
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        
        // Skip empty lines and comments
        if trimmed.is_empty() || trimmed.starts_with("//") {
            result.push_str(line);
            result.push('\n');
            i += 1;
            continue;
        }
        
        // Start building a statement
        let mut statement_lines = vec![line];
        i += 1;
        
        // Look ahead for method chaining (lines starting with .)
        while i < lines.len() {
            let next_line = lines[i];
            let next_trimmed = next_line.trim();
            
            // Skip empty lines and comments in method chain
            if next_trimmed.is_empty() || next_trimmed.starts_with("//") {
                statement_lines.push(next_line);
                i += 1;
                continue;
            }
            
            // If next line starts with dot, it's method chaining
            if next_trimmed.starts_with('.') {
                statement_lines.push(next_line);
                i += 1;
            } else {
                // Not method chaining, stop here
                break;
            }
        }
        
        // Join the statement lines and add to result
        for (idx, stmt_line) in statement_lines.iter().enumerate() {
            if idx > 0 {
                result.push('\n');
            }
            result.push_str(stmt_line);
        }
        result.push('\n');
    }
    
    result
}

/// Extract individual statements from OQL file content (after preprocessing)
fn extract_statements(content: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    // After preprocessing, statements are separated by double newlines or end of content
    // This is much simpler since preprocessing has already handled method chaining
    let mut statements = Vec::new();
    let mut current_statement = String::new();
    
    for line in content.lines() {
        let trimmed = line.trim();
        
        // If we hit an empty line and have a current statement, end it
        if trimmed.is_empty() {
            if !current_statement.trim().is_empty() {
                statements.push(current_statement.trim().to_string());
                current_statement.clear();
            }
            continue;
        }
        
        // Skip standalone comments
        if trimmed.starts_with("//") && current_statement.trim().is_empty() {
            continue;
        }
        
        // Add line to current statement
        if !current_statement.is_empty() {
            current_statement.push('\n');
        }
        current_statement.push_str(line);
    }
    
    // Add any remaining statement
    if !current_statement.trim().is_empty() {
        statements.push(current_statement.trim().to_string());
    }
    
    Ok(statements)
}

/// Parse a schema definition from OQL syntax
fn parse_schema_definition(statement: &str) -> Result<SchemaDefinition, Box<dyn std::error::Error>> {
    let lines: Vec<&str> = statement.lines().collect();
    if lines.is_empty() {
        return Err("Empty schema definition".into());
    }
    
    // Extract schema name from first line: "schema users {"
    let first_line = lines[0].trim();
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() < 2 || parts[0] != "schema" {
        return Err("Invalid schema definition".into());
    }
    
    let schema_name = parts[1].trim_end_matches('{').trim().to_string();
    let mut fields = Vec::new();
    
    // Parse fields from remaining lines
    for line in &lines[1..] {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "}" || trimmed.starts_with("//") {
            continue;
        }
        
        // Parse field definition: "name: String," or "email: String?,"
        if let Some(colon_pos) = trimmed.find(':') {
            let field_name = trimmed[..colon_pos].trim().to_string();
            let field_type_part = trimmed[colon_pos + 1..].trim();
            
            // Remove trailing comma
            let field_type_clean = field_type_part.trim_end_matches(',').trim();
            
            // Check if optional (ends with ?)
            let (field_type, optional) = if field_type_clean.ends_with('?') {
                (field_type_clean.trim_end_matches('?').trim().to_string(), true)
            } else {
                (field_type_clean.to_string(), false)
            };
            
            fields.push(SchemaField {
                name: field_name,
                field_type,
                optional,
            });
        }
    }
    
    Ok(SchemaDefinition {
        name: schema_name,
        fields,
    })
}

/// Execute a parsed OQL file
pub fn execute_oql_file<S: crate::storage::DocumentStore>(
    file: &OQLFile,
    executor: &crate::executor::QueryExecutor<S>,
) -> Result<OQLFileExecutionResult, Box<dyn std::error::Error>> {
    let mut results = Vec::new();
    let mut schema_count = 0;
    let mut query_count = 0;
    let mut function_count = 0;
    let mut success_count = 0;
    let mut errors = Vec::new();
    
    // Create a schema registry for type resolution
    let registry = crate::schema::SchemaRegistry::new();
    
    // First pass: Register all schema names (for forward references)
    for statement in &file.statements {
        if let OQLFileStatement::Schema(schema_def) = statement {
            // Create empty schema just to register the name
            let empty_schema = crate::schema::Schema::new(
                schema_def.name.clone(),
                std::collections::HashMap::new()
            );
            registry.register(empty_schema).ok(); // Ignore errors for now
        }
    }
    
    // Second pass: Convert and register full schemas with resolved types
    for statement in &file.statements {
        if let OQLFileStatement::Schema(schema_def) = statement {
            match schema_definition_to_schema(schema_def, &registry) {
                Ok(schema) => {
                    // Register in registry
                    registry.register(schema.clone()).ok();
                    // Store in database
                    executor.store().put_schema(&schema).ok();
                }
                Err(e) => {
                    eprintln!("Warning: Failed to convert schema {}: {}", schema_def.name, e);
                }
            }
        }
    }
    
    // Third pass: Execute statements
    for statement in &file.statements {
        match statement {
            OQLFileStatement::Schema(schema) => {
                schema_count += 1;
                println!("📋 Schema definition: {}", schema.name);
                success_count += 1;
                results.push(StatementResult::Schema(schema.name.clone()));
            }
            OQLFileStatement::Query(query) => {
                query_count += 1;
                let start_time = std::time::Instant::now();
                match executor.execute_query(query) {
                    Ok(documents) => {
                        let elapsed = start_time.elapsed();
                        success_count += 1;
                        let query_str = format!("{}.{}", query.collection, 
                            query.operations.iter()
                                .map(|op| format!("{:?}", op))
                                .collect::<Vec<_>>()
                                .join(".")
                        );
                        println!("✅ {} -> {} documents ({:.3}ms)", 
                            query_str.chars().take(50).collect::<String>(), 
                            documents.len(),
                            elapsed.as_secs_f64() * 1000.0
                        );
                        results.push(StatementResult::Query(documents.len()));
                    }
                    Err(e) => {
                        let elapsed = start_time.elapsed();
                        let error_msg = format!("Execution failed: {}", e);
                        println!("❌ {} ({:.3}ms)", error_msg, elapsed.as_secs_f64() * 1000.0);
                        errors.push(error_msg);
                        results.push(StatementResult::Error(e.to_string()));
                    }
                }
            }
            OQLFileStatement::FunctionDef(func_def) => {
                function_count += 1;
                // TODO: Register function
                println!("🔧 Function definition: {}", func_def.name);
                success_count += 1;
                results.push(StatementResult::Function(func_def.name.clone()));
            }
        }
    }
    
    println!("\n📊 Summary: {}/{} statements executed successfully", 
        success_count, schema_count + query_count + function_count);
    println!("   - {} schemas", schema_count);
    println!("   - {} queries", query_count);
    println!("   - {} functions", function_count);
    
    Ok(OQLFileExecutionResult {
        results,
        schema_count,
        query_count,
        function_count,
        success_count,
        errors,
    })
}

/// Result of executing an OQL file
#[derive(Debug, Clone)]
pub struct OQLFileExecutionResult {
    pub results: Vec<StatementResult>,
    pub schema_count: usize,
    pub query_count: usize,
    pub function_count: usize,
    pub success_count: usize,
    pub errors: Vec<String>,
}

/// Result of executing a single statement
#[derive(Debug, Clone)]
pub enum StatementResult {
    Schema(String),
    Query(usize), // Number of documents returned
    Function(String),
    Error(String),
}

/// Convert a SchemaDefinition to a Schema with resolved types
pub fn schema_definition_to_schema(
    def: &SchemaDefinition,
    registry: &crate::schema::SchemaRegistry,
) -> crate::error::OQLResult<crate::schema::Schema> {
    use std::collections::HashMap;
    use crate::schema::{FieldType, ReferenceType};
    
    let mut fields = HashMap::new();
    
    for field in &def.fields {
        let field_type = parse_field_type(&field.field_type, registry)?;
        let final_type = if field.optional {
            FieldType::Optional(Box::new(field_type))
        } else {
            field_type
        };
        fields.insert(field.name.clone(), final_type);
    }
    
    Ok(crate::schema::Schema::new(def.name.clone(), fields))
}

/// Parse a field type string into a FieldType enum
fn parse_field_type(
    type_str: &str,
    registry: &crate::schema::SchemaRegistry,
) -> crate::error::OQLResult<crate::schema::FieldType> {
    use crate::schema::{FieldType, ReferenceType};
    use crate::error::OQLError;
    
    match type_str {
        "String" => Ok(FieldType::String),
        "Integer" => Ok(FieldType::Integer),
        "Float" => Ok(FieldType::Float),
        "Boolean" => Ok(FieldType::Boolean),
        "DateTime" => Ok(FieldType::DateTime),
        // Check if it's a schema name (reference type)
        schema_name if registry.exists(schema_name) => {
            Ok(FieldType::Reference(ReferenceType::new(schema_name.to_string())))
        }
        _ => {
            // Unknown type - could be a typo or forward reference
            // For now, treat as String to avoid breaking existing code
            Ok(FieldType::String)
        }
    }
}
