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
    Upsert,
    Update,
    Delete,
    Sort,
    Limit,
    Offset,
    First,
    Exists,
    GroupBy,
    Having,
    Nearest,
    Distinct,
    Explain,

    // Function definition
    Fn,
    Arrow, // ->
    Let,
    On,

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
    NullLiteral,

    // Identifiers and field access
    Identifier(String),       // collection names, variable names
    FieldAccess(Vec<String>), // .field or .field.subfield

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
    Assign,       // =

    // Special
    Whitespace,
    Comment(String),
    EOF,
}

/// The name the parser gives "the rows this expression is evaluated over"
/// when an operation is written with no receiver. It is not a legal
/// identifier, so it cannot collide with a binding or a field.
pub const CURRENT_ROWS: &str = "$rows";

/// The marker the parser gives a `select` entry written `-name`: the named
/// field is left out of the result.
pub const OMITTED_FIELD: &str = "$omit";

impl Token {
    /// The source word of a keyword token.
    ///
    /// Keywords are contextual: where the grammar expects a name (after a
    /// `.`, before a `:`, as a projected field) a keyword is just a name, so
    /// data may have fields called `count`, `first` or `limit`.
    fn keyword_text(&self) -> Option<&'static str> {
        Some(match self {
            Token::Where => "where",
            Token::Select => "select",
            Token::Insert => "insert",
            Token::Upsert => "upsert",
            Token::Update => "update",
            Token::Delete => "delete",
            Token::Sort => "sort",
            Token::Limit => "limit",
            Token::Offset => "offset",
            Token::First => "first",
            Token::Exists => "exists",
            Token::GroupBy => "group_by",
            Token::Having => "having",
            Token::Nearest => "nearest",
            Token::Distinct => "distinct",
            Token::Explain => "explain",
            Token::Fn => "fn",
            Token::Let => "let",
            Token::On => "on",
            Token::Count => "count",
            Token::Sum => "sum",
            Token::Avg => "avg",
            Token::Min => "min",
            Token::Max => "max",
            Token::If => "if",
            Token::Then => "then",
            Token::Else => "else",
            Token::Match => "match",
            Token::Contains => "contains",
            Token::StartsWith => "starts_with",
            Token::EndsWith => "ends_with",
            Token::In => "in",
            _ => return None,
        })
    }

    /// Whether this keyword names a collection operation (`.where { .. }`,
    /// `.count()`), as opposed to an operator or a structural word.
    fn is_operation_keyword(&self) -> bool {
        matches!(
            self,
            Token::Where
                | Token::Select
                | Token::Insert
                | Token::Upsert
                | Token::Update
                | Token::Delete
                | Token::Sort
                | Token::Limit
                | Token::Offset
                | Token::First
                | Token::Exists
                | Token::GroupBy
                | Token::Having
                | Token::Nearest
                | Token::Distinct
                | Token::Explain
                | Token::Count
                | Token::Sum
                | Token::Avg
                | Token::Min
                | Token::Max
        )
    }
}

// Tokenizer implementation
pub struct Tokenizer;

impl Tokenizer {
    pub fn tokenize(input: &str) -> Result<Vec<Token>, TokenizeError> {
        Self::tokenize_with_offsets(input).map(|(tokens, _)| tokens)
    }

    /// Tokenize, also returning the byte offset in `input` at which each
    /// token starts, so an error can point at the user's own text.
    pub fn tokenize_with_offsets(input: &str) -> Result<(Vec<Token>, Vec<usize>), TokenizeError> {
        let mut tokens = Vec::new();
        let mut offsets = Vec::new();
        let mut remaining = input;

        while !remaining.is_empty() {
            match Self::next_token(remaining) {
                Ok((rest, token)) => {
                    if !matches!(token, Token::Whitespace | Token::Comment(_)) {
                        tokens.push(token);
                        offsets.push(input.len() - remaining.len());
                    }
                    remaining = rest;
                }
                Err(_) => {
                    // Report where the input stopped making sense, in the
                    // user's own text, rather than a parser-internal trace.
                    let line = input[..input.len() - remaining.len()]
                        .matches('\n')
                        .count()
                        + 1;
                    if remaining.starts_with('"') {
                        return Err(TokenizeError::ParseError(format!(
                            "line {line}: string literal is unterminated or contains an \
                             unknown escape (use \\\\ for a backslash and \\\" for a quote)"
                        )));
                    }
                    let snippet: String = remaining.chars().take(24).collect();
                    return Err(TokenizeError::ParseError(format!(
                        "line {line}: unexpected input at `{}`",
                        snippet.lines().next().unwrap_or_default()
                    )));
                }
            }
        }

        tokens.push(Token::EOF);
        offsets.push(input.len());
        Ok((tokens, offsets))
    }

    fn next_token(input: &str) -> IResult<&str, Token> {
        alt((
            Self::parse_whitespace,
            Self::parse_comment,
            Self::parse_operator,
            Self::parse_string_literal,
            Self::parse_number,
            Self::parse_boolean,
            Self::parse_null,
            Self::parse_parameter,
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
                tag("nearest"),
                tag("distinct"),
                tag("where"),
                tag("select"),
                tag("insert"),
                tag("upsert"),
                tag("update"),
                tag("delete"),
                tag("sort"),
                tag("limit"),
                tag("offset"),
                tag("first"),
                tag("exists"),
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
                tag("explain"),
                tag("fn"),
                tag("let"),
                tag("on"),
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

        // Check that the keyword is followed by a non-identifier character (or end of input).
        // Underscore is part of identifiers, so keywords must not split `avg_amount`, etc.
        if let Some(next) = input.chars().next() {
            if next.is_ascii_alphanumeric() || next == '_' {
                // This is part of a larger identifier, not a standalone keyword.
                return Err(nom::Err::Error(nom::error::Error::new(
                    input,
                    nom::error::ErrorKind::Tag,
                )));
            }
        }

        let token = match keyword {
            "group_by" => Token::GroupBy,
            "having" => Token::Having,
            "nearest" => Token::Nearest,
            "distinct" => Token::Distinct,
            "explain" => Token::Explain,
            "where" => Token::Where,
            "select" => Token::Select,
            "insert" => Token::Insert,
            "upsert" => Token::Upsert,
            "update" => Token::Update,
            "delete" => Token::Delete,
            "sort" => Token::Sort,
            "limit" => Token::Limit,
            "offset" => Token::Offset,
            "first" => Token::First,
            "exists" => Token::Exists,
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
            "on" => Token::On,
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
            map(tag("!"), |_| Token::Not),
            map(tag("<="), |_| Token::LessEqual),
            map(tag(">="), |_| Token::GreaterEqual),
            map(tag("="), |_| Token::Assign),
            map(tag("<"), |_| Token::LessThan),
            map(tag(">"), |_| Token::GreaterThan),
            map(tag("+"), |_| Token::Plus),
            map(tag("-"), |_| Token::Minus),
            map(tag("/"), |_| Token::Slash),
            map(tag("%"), |_| Token::Percent),
        ))(input)
    }

    /// A double-quoted string. A backslash escapes the next character:
    /// `\"` and `\\` stand for themselves, `\n`, `\r`, `\t` and `\0` are
    /// the usual control characters, and `\u{1F989}` is a Unicode scalar.
    /// Any other escape is an error rather than a silent guess.
    fn parse_string_literal(input: &str) -> IResult<&str, Token> {
        let (body, _) = char('"')(input)?;
        let fail = |at: &'static str| {
            let _ = at;
            nom::Err::Failure(nom::error::Error::new(input, nom::error::ErrorKind::Escaped))
        };
        let mut content = String::new();
        let mut chars = body.char_indices();
        while let Some((offset, character)) = chars.next() {
            match character {
                '"' => {
                    return Ok((&body[offset + 1..], Token::StringLiteral(content)));
                }
                '\\' => match chars.next().map(|(_, escaped)| escaped) {
                    Some('"') => content.push('"'),
                    Some('\\') => content.push('\\'),
                    Some('/') => content.push('/'),
                    Some('n') => content.push('\n'),
                    Some('r') => content.push('\r'),
                    Some('t') => content.push('\t'),
                    Some('0') => content.push('\0'),
                    Some('u') => {
                        if chars.next().map(|(_, open)| open) != Some('{') {
                            return Err(fail("unicode escape"));
                        }
                        let mut digits = String::new();
                        loop {
                            match chars.next().map(|(_, digit)| digit) {
                                Some('}') => break,
                                Some(digit) if digit.is_ascii_hexdigit() && digits.len() < 6 => {
                                    digits.push(digit)
                                }
                                _ => return Err(fail("unicode escape")),
                            }
                        }
                        let scalar = u32::from_str_radix(&digits, 16)
                            .ok()
                            .and_then(char::from_u32)
                            .ok_or_else(|| fail("unicode escape"))?;
                        content.push(scalar);
                    }
                    _ => return Err(fail("string escape")),
                },
                other => content.push(other),
            }
        }
        Err(fail("unterminated string"))
    }

    fn parse_number(input: &str) -> IResult<&str, Token> {
        alt((Self::parse_float, Self::parse_integer))(input)
    }

    fn parse_float(input: &str) -> IResult<&str, Token> {
        let (input, number_str) = recognize(tuple((digit1, char('.'), digit1)))(input)?;

        let value = number_str.parse::<f64>().map_err(|_| {
            nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Float))
        })?;

        Ok((input, Token::FloatLiteral(value)))
    }

    fn parse_integer(input: &str) -> IResult<&str, Token> {
        let (input, number_str) = digit1(input)?;
        let value = number_str.parse::<i64>().map_err(|_| {
            nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Digit))
        })?;

        Ok((input, Token::IntegerLiteral(value)))
    }

    fn parse_boolean(input: &str) -> IResult<&str, Token> {
        let (rest, token) = alt((
            map(tag("true"), |_| Token::BooleanLiteral(true)),
            map(tag("false"), |_| Token::BooleanLiteral(false)),
        ))(input)?;
        // `true_count` and `falsey` are identifiers, not a literal followed
        // by more text.
        if rest
            .chars()
            .next()
            .is_some_and(|next| next.is_ascii_alphanumeric() || next == '_')
        {
            return Err(nom::Err::Error(nom::error::Error::new(
                input,
                nom::error::ErrorKind::Tag,
            )));
        }
        Ok((rest, token))
    }

    /// A query parameter such as `$from`. It is carried as a name whose
    /// value the host supplies before execution (see [`bind_parameters`]),
    /// so values never have to be spliced into query text.
    fn parse_parameter(input: &str) -> IResult<&str, Token> {
        let (rest, _) = char('$')(input)?;
        let (rest, name) = Self::parse_identifier_name(rest)?;
        Ok((rest, Token::Identifier(format!("${name}"))))
    }

    fn parse_null(input: &str) -> IResult<&str, Token> {
        let (rest, _) = tag("null")(input)?;
        if rest
            .chars()
            .next()
            .is_some_and(|next| next.is_ascii_alphanumeric() || next == '_')
        {
            return Err(nom::Err::Error(nom::error::Error::new(
                input,
                nom::error::ErrorKind::Tag,
            )));
        }
        Ok((rest, Token::NullLiteral))
    }

    fn parse_field_access(input: &str) -> IResult<&str, Token> {
        let (input, _) = char('.')(input)?;
        let (input, first_field) = Self::parse_path_segment(input)?;
        let (input, additional_fields) =
            many0(preceded(char('.'), Self::parse_path_segment))(input)?;

        let mut path = vec![first_field];
        path.extend(additional_fields);

        Ok((input, Token::FieldAccess(path)))
    }

    /// One name in a dotted path. Any identifier is a valid field name,
    /// including a keyword, unless the text is an operation being applied:
    /// `.count` is the field `count`, while `.count()` counts.
    fn parse_path_segment(input: &str) -> IResult<&str, String> {
        if Self::is_operation_applied(input) {
            return Err(nom::Err::Error(nom::error::Error::new(
                input,
                nom::error::ErrorKind::Tag,
            )));
        }
        Self::parse_identifier_name(input)
    }

    /// True when `input` starts with an operation keyword followed by what
    /// that operation takes: a block, an argument list, a list of documents
    /// for `insert`, `on` for `upsert`, or a bare count for `limit`/`offset`.
    fn is_operation_applied(input: &str) -> bool {
        let Ok((rest, token)) = Self::parse_keyword(input) else {
            return false;
        };
        if !token.is_operation_keyword() {
            return false;
        }
        let rest = rest.trim_start();
        match rest.chars().next() {
            // `users.where { .. }.delete` with nothing after it is the
            // long-standing bare form of `delete()`.
            None => token == Token::Delete,
            Some('{' | '(') => true,
            Some('[') => token == Token::Insert,
            Some(next) if next.is_ascii_digit() => matches!(token, Token::Limit | Token::Offset),
            Some('o') => {
                token == Token::Upsert
                    && rest.strip_prefix("on").is_some_and(|after| {
                        !after
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
                    })
            }
            _ => false,
        }
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

        assert_eq!(
            tokens,
            vec![
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
            ]
        );
    }

    #[test]
    fn test_tokenize_operators() {
        let input = "== != < > <= >=";
        let tokens = Tokenizer::tokenize(input).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Equal,
                Token::NotEqual,
                Token::LessThan,
                Token::GreaterThan,
                Token::LessEqual,
                Token::GreaterEqual,
                Token::EOF,
            ]
        );
    }

    #[test]
    fn test_tokenize_literals() {
        let input = r#""hello world" 42 3.14 true false"#;
        let tokens = Tokenizer::tokenize(input).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::StringLiteral("hello world".to_string()),
                Token::IntegerLiteral(42),
                Token::FloatLiteral(3.14),
                Token::BooleanLiteral(true),
                Token::BooleanLiteral(false),
                Token::EOF,
            ]
        );
    }

    #[test]
    fn test_tokenize_identifiers_and_field_access() {
        let input = "customers .name .customer.account.token user_id";
        let tokens = Tokenizer::tokenize(input).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Identifier("customers".to_string()),
                Token::FieldAccess(vec!["name".to_string()]),
                Token::FieldAccess(vec![
                    "customer".to_string(),
                    "account".to_string(),
                    "token".to_string()
                ]),
                Token::Identifier("user_id".to_string()),
                Token::EOF,
            ]
        );
    }

    #[test]
    fn test_tokenize_punctuation() {
        let input = "{ } ( ) , : .";
        let tokens = Tokenizer::tokenize(input).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::LeftBrace,
                Token::RightBrace,
                Token::LeftParen,
                Token::RightParen,
                Token::Comma,
                Token::Colon,
                Token::Dot,
                Token::EOF,
            ]
        );
    }

    #[test]
    fn test_tokenize_oql_query() {
        let input = r#"customers.where { .name == "Alice" }"#;
        let tokens = Tokenizer::tokenize(input).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Identifier("customers".to_string()),
                Token::Dot,
                Token::Where,
                Token::LeftBrace,
                Token::FieldAccess(vec!["name".to_string()]),
                Token::Equal,
                Token::StringLiteral("Alice".to_string()),
                Token::RightBrace,
                Token::EOF,
            ]
        );
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
    InsertMany(Vec<InsertBlock>), // Batch insert multiple documents
    Upsert(UpsertBlock),
    Update(UpdateBlock),
    Delete,
    Sort(SortBlock),
    Limit(u64),
    Offset(u64),
    /// `limit(expr)` whose count is not a literal, typically a `$parameter`.
    /// It is folded into [`Operation::Limit`] as soon as the value is known.
    LimitExpr(Expression),
    /// `offset(expr)`; see [`Operation::LimitExpr`].
    OffsetExpr(Expression),
    /// `distinct()`: keep one row of each set of rows that are equal.
    Distinct,
    /// `explain()`: instead of running the query, report how it would run.
    Explain,
    /// `sum(expr)`, `avg(expr)`, `min(expr)` or `max(expr)` over a computed
    /// value rather than a stored path, as in `.sum(.price * .qty)`.
    AggregateExpr {
        function: String,
        input: Expression,
    },
    First,
    Exists,
    Nearest(NearestBlock),
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
pub struct NearestBlock {
    pub field: FieldAccess,
    pub query: Expression,
    pub metric: VectorMetric,
    pub k: u64,
    pub num_candidates: Option<u64>,
    pub exact: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VectorMetric {
    Cosine,
    L2,
    Dot,
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
pub struct UpsertBlock {
    pub key: String,
    pub fields: Vec<InsertField>,
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
    /// Keys computed from an expression, such as
    /// `month: format_date(.placed, "%Y-%m")`. Each has the name its value
    /// takes in the group row.
    #[serde(default)]
    pub computed: Vec<ComputedGroupKey>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComputedGroupKey {
    pub name: String,
    pub expression: Expression,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HavingBlock {
    pub condition: Condition,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expression {
    /// A bare name. Resolution decides whether it is a script binding,
    /// block-local value, function parameter, or collection scope anchor.
    Identifier(String),
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
    ConditionExpr(Box<Condition>),
    QueryExpression {
        query: OQLQuery,
    },
    /// Collection operations rooted in a value expression, such as
    /// `.items.where { ... }` or `.findings.count()`.
    CollectionExpression {
        source: Box<Expression>,
        operations: Vec<Operation>,
    },
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
    pub path: Vec<String>, // ["field", "subfield", "nested"]
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
    Null,
}

// Parser implementation for OQL method-chaining syntax
pub struct Parser {
    tokens: Vec<Token>,
    position: usize,
    schema_registry: Option<crate::schema::SchemaRegistry>,
    /// How many expressions and conditions are currently being parsed
    /// inside one another.
    depth: usize,
}

/// Nesting deeper than this is reported as an error. The parser is
/// recursive, so unbounded nesting would exhaust the stack instead.
const MAX_NESTING_DEPTH: usize = 64;

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            position: 0,
            schema_registry: None,
            depth: 0,
        }
    }

    /// Create a new parser with schema registry for validation
    pub fn with_schema_registry(
        tokens: Vec<Token>,
        schema_registry: crate::schema::SchemaRegistry,
    ) -> Self {
        Self {
            tokens,
            position: 0,
            schema_registry: Some(schema_registry),
            depth: 0,
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

            // Vector operators (exact-match only for now)
            (FieldType::Vector { .. }, ComparisonOperator::Equal) => Ok(()),
            (FieldType::Vector { .. }, ComparisonOperator::NotEqual) => Ok(()),

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
                if ref1.target_schema == ref2.target_schema =>
            {
                Ok(())
            }

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

            // Vector types - dimensions must match
            (FieldType::Vector { dim: left_dim }, FieldType::Vector { dim: right_dim })
                if left_dim == right_dim =>
            {
                Ok(())
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
            Expression::Identifier(name) => self.infer_expression_type(
                &Expression::FieldAccess(FieldAccess {
                    path: vec![name.clone()],
                }),
                schema_name,
            ),
            Expression::Literal(lit) => Ok(match lit {
                Literal::String(_) => FieldType::String,
                Literal::Integer(_) => FieldType::Integer,
                Literal::Float(_) => FieldType::Float,
                Literal::Boolean(_) => FieldType::Boolean,
                // Null has no standalone schema type. This placeholder is
                // only used for projected null values; comparisons and writes
                // handle null explicitly below.
                Literal::Null => FieldType::Optional(Box::new(FieldType::Object)),
            }),

            Expression::FieldAccess(field_access) => {
                if let Some(registry) = &self.schema_registry {
                    // Get the field type from schema
                    let mut current_schema = schema_name.to_string();
                    let mut current_type: Option<FieldType> = None;

                    for field_name in &field_access.path {
                        let field_type = registry
                            .get_field_type(&current_schema, field_name)
                            .map_err(|e| ParseError::SchemaError {
                                message: e.to_string(),
                            })?
                            .ok_or_else(|| ParseError::SchemaError {
                                message: format!(
                                    "Field '{}' not found in schema '{}'",
                                    field_name, current_schema
                                ),
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
                                message: format!(
                                    "Function '{}' requires at least one argument",
                                    name
                                ),
                            })
                        } else {
                            self.infer_expression_type(&args[0], schema_name)
                        }
                    }
                    "length" | "len" => Ok(FieldType::Integer),
                    "exists" | "is_null" | "is_missing" | "is_broken_ref" => Ok(FieldType::Boolean),
                    "upper" | "lower" | "trim" => Ok(FieldType::String),
                    "now" => Ok(FieldType::DateTime),
                    _ => Err(ParseError::TypeError {
                        message: format!("Unknown function '{}'", name),
                    }),
                }
            }

            Expression::BinaryOp {
                left,
                operator,
                right,
            } => {
                let left_type = self.infer_expression_type(left, schema_name)?;
                let right_type = self.infer_expression_type(right, schema_name)?;

                match operator {
                    BinaryOperator::Add
                    | BinaryOperator::Subtract
                    | BinaryOperator::Multiply
                    | BinaryOperator::Divide
                    | BinaryOperator::Modulo => {
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

            Expression::IfThenElse {
                then_expr,
                else_expr,
                ..
            } => {
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
            Expression::ObjectLiteral(_) => Ok(FieldType::Object),
            Expression::ConditionExpr(_) => Ok(FieldType::Boolean),
            Expression::QueryExpression { query } => {
                if matches!(query.operations.last(), Some(Operation::Count)) {
                    Ok(FieldType::Integer)
                } else {
                    Ok(FieldType::List(Box::new(FieldType::Object)))
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
            Condition::Comparison {
                left,
                operator,
                right,
            } => {
                if matches!(
                    (left, right),
                    (
                        Expression::Literal(Literal::Null),
                        Expression::Literal(Literal::Null)
                    ) | (Expression::Literal(Literal::Null), _)
                        | (_, Expression::Literal(Literal::Null))
                ) {
                    return match operator {
                        ComparisonOperator::Equal | ComparisonOperator::NotEqual => Ok(()),
                        _ => Err(ParseError::TypeError {
                            message: "null only supports == and != comparisons".to_string(),
                        }),
                    };
                }

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
                let cost = if s1.chars().nth(i - 1) == s2.chars().nth(j - 1) {
                    0
                } else {
                    1
                };
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
                let mut suggestions: Vec<(String, usize)> = schema
                    .fields
                    .keys()
                    .map(|f| (f.clone(), Self::levenshtein_distance(field_name, f)))
                    .filter(|(_, dist)| *dist <= 3) // Only suggest if distance is 3 or less
                    .collect();

                suggestions.sort_by_key(|(_, dist)| *dist);
                return suggestions
                    .into_iter()
                    .take(3)
                    .map(|(name, _)| name)
                    .collect();
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
                Ok(_) => {}
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

                    return Err(ParseError::SchemaError { message: error_msg });
                }
            }

            // Additionally validate reference targets if this is a reference field
            if field_access.path.len() > 1 {
                // For nested field access, validate each reference in the chain
                let mut current_schema = schema_name.to_string();

                for (i, field_name) in field_access.path.iter().enumerate() {
                    if i < field_access.path.len() - 1 {
                        // This is not the last field, so it should be a reference
                        let field_type = registry
                            .get_field_type(&current_schema, field_name)
                            .map_err(|e| ParseError::SchemaError {
                                message: e.to_string(),
                            })?
                            .ok_or_else(|| ParseError::SchemaError {
                                message: format!(
                                    "Field '{}' not found in schema '{}'",
                                    field_name, current_schema
                                ),
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
            Expression::Identifier(name) => {
                self.validate_field_exists(
                    &FieldAccess {
                        path: vec![name.clone()],
                    },
                    schema_name,
                )?;
            }
            Expression::FieldAccess(field_access) => {
                self.validate_field_exists(field_access, schema_name)?;
            }
            Expression::FunctionCall { args, .. } => {
                for arg in args {
                    self.validate_expression_fields(arg, schema_name)?;
                }
            }
            Expression::IfThenElse {
                condition,
                then_expr,
                else_expr,
            } => {
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
            Expression::ConditionExpr(condition) => {
                self.validate_condition_fields(condition, schema_name)?;
            }
            Expression::QueryExpression { query } => {
                self.validate_query_expression(query)?;
            }
            Expression::CollectionExpression { source, operations } => {
                self.validate_expression_fields(source, schema_name)?;
                for operation in operations {
                    match operation {
                        Operation::Where(block) => {
                            self.validate_condition_fields(&block.condition, schema_name)?
                        }
                        Operation::Select(block) => {
                            for field in &block.fields {
                                self.validate_expression_fields(&field.expression, schema_name)?;
                            }
                        }
                        _ => {}
                    }
                }
            }
            Expression::Literal(_) => {
                // Literals don't have fields to validate
            }
        }
        Ok(())
    }

    fn validate_query_expression(&self, query: &OQLQuery) -> Result<(), ParseError> {
        if let Some(registry) = &self.schema_registry {
            if !registry.exists(&query.collection) {
                return Err(ParseError::SchemaError {
                    message: format!("Schema '{}' not found", query.collection),
                });
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
                        message: format!(
                            "Function '{}' requires at least one argument",
                            function_name
                        ),
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
                        message: format!(
                            "Function '{}' requires at least one argument",
                            function_name
                        ),
                    });
                }

                let arg_type = self.infer_expression_type(&args[0], schema_name)?;
                match arg_type {
                    FieldType::Integer
                    | FieldType::Float
                    | FieldType::DateTime
                    | FieldType::Optional(_) => Ok(()),
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
                        message: format!(
                            "Function '{}' requires at least one argument",
                            function_name
                        ),
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
                        message: format!(
                            "Function '{}' requires at least one argument",
                            function_name
                        ),
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
            "exists" | "is_null" | "is_missing" | "is_broken_ref" => {
                if args.len() != 1 {
                    return Err(ParseError::TypeError {
                        message: format!(
                            "Function '{}' requires exactly one path argument",
                            function_name
                        ),
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
            Some(token) => {
                return Err(ParseError::UnexpectedToken(format!(
                    "Expected function name, found {:?}",
                    token
                )))
            }
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
                Some(token) => {
                    return Err(ParseError::UnexpectedToken(format!(
                        "Expected return type, found {:?}",
                        token
                    )))
                }
                None => return Err(ParseError::UnexpectedEndOfInput),
            }
        } else {
            None
        };

        // Parse function body: any number of local names, then the value.
        self.expect_token(Token::LeftBrace)?;
        let mut locals: std::collections::HashMap<String, Expression> =
            std::collections::HashMap::new();
        while let (Some(Token::Identifier(local)), Some(Token::Assign)) =
            (self.current_token(), self.peek_token())
        {
            let local = local.clone();
            if parameters.iter().any(|parameter| parameter.name == local) {
                return Err(ParseError::InvalidSyntax(format!(
                    "local name '{local}' in function '{name}' would hide the parameter of \
                     the same name"
                )));
            }
            if locals.contains_key(&local) {
                return Err(ParseError::InvalidSyntax(format!(
                    "local name '{local}' is already defined in function '{name}'"
                )));
            }
            self.advance();
            self.advance();
            // A local is a name for its expression, exactly as in a scoped
            // query block: later lines may use the names before them.
            let value = self.parse_expression()?;
            let value = Self::substitute_scoped_locals_expression(&value, &locals);
            locals.insert(local, value);
        }
        let body = if return_type
            .as_ref()
            .map(|t| Self::is_boolean_type_name(t))
            .unwrap_or(false)
        {
            Expression::ConditionExpr(Box::new(Self::substitute_scoped_locals_condition(
                &self.parse_condition()?,
                &locals,
            )))
        } else {
            Self::substitute_scoped_locals_expression(&self.parse_expression()?, &locals)
        };
        self.expect_token(Token::RightBrace)?;

        Ok(FunctionDefinition {
            name,
            parameters,
            return_type,
            body,
        })
    }

    fn is_boolean_type_name(type_name: &str) -> bool {
        matches!(type_name.to_ascii_lowercase().as_str(), "bool" | "boolean")
    }

    fn parse_function_parameter(&mut self) -> Result<FunctionParameter, ParseError> {
        // Parse parameter name
        let name = match self.current_token() {
            Some(Token::Identifier(name)) => {
                let name = name.clone();
                self.advance();
                name
            }
            Some(token) => {
                return Err(ParseError::UnexpectedToken(format!(
                    "Expected parameter name, found {:?}",
                    token
                )))
            }
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
            Some(token) => {
                return Err(ParseError::UnexpectedToken(format!(
                    "Expected parameter type, found {:?}",
                    token
                )))
            }
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
            Some(token) => {
                return Err(ParseError::UnexpectedToken(format!(
                    "Expected collection name, found {:?}",
                    token
                )))
            }
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

        // Scoped blocks and chained queries deliberately lower to the same
        // ordered operation list. There is one semantic query model.
        if matches!(self.current_token(), Some(Token::LeftBrace)) {
            return self.parse_scoped_query(collection);
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
            return Err(ParseError::UnexpectedToken(format!(
                "Expected end of query, found {:?}",
                self.current_token()
            )));
        }

        Ok(OQLQuery {
            collection,
            operations,
        })
    }

    fn parse_scoped_query(&mut self, collection: String) -> Result<OQLQuery, ParseError> {
        self.expect_token(Token::LeftBrace)?;

        let mut operations = Vec::new();
        let mut locals: std::collections::HashMap<String, Expression> =
            std::collections::HashMap::new();
        // The entry that produces the block's result: a `select`, or any
        // terminal the chained form allows (`count()`, `first()`, an
        // aggregate, `update { .. }`, `delete()`).
        let mut result_entry: Option<&'static str> = None;

        while !matches!(self.current_token(), Some(Token::RightBrace)) {
            if matches!(self.current_token(), Some(Token::EOF) | None) {
                return Err(ParseError::UnexpectedEndOfInput);
            }

            // A block-local definition is syntactically `name = expression`.
            // It is expanded into the canonical expression tree here. This
            // keeps scoped and chained forms on one planner/executor path.
            if let (Some(Token::Identifier(name)), Some(Token::Assign)) =
                (self.current_token(), self.tokens.get(self.position + 1))
            {
                let name = name.clone();
                if name == collection {
                    return Err(ParseError::InvalidSyntax(format!(
                        "Scoped binding '{}' cannot shadow the root collection",
                        name
                    )));
                }
                if locals.contains_key(&name) {
                    return Err(ParseError::InvalidSyntax(format!(
                        "Scoped binding '{}' is already defined",
                        name
                    )));
                }

                self.advance();
                self.expect_token(Token::Assign)?;
                let value = self.parse_expression()?;
                let value = Self::substitute_scoped_locals_expression(&value, &locals);
                locals.insert(name, value);
                continue;
            }

            if let Some(entry) = result_entry {
                return Err(ParseError::InvalidSyntax(format!(
                    "{entry} must be the final entry in a scoped query block"
                )));
            }

            let operation = self.parse_operation()?;
            let operation = Self::substitute_scoped_locals_operation(&operation, &locals);
            self.validate_parsed_operation(&operation, &collection)?;
            result_entry = match operation {
                Operation::Select(_) => Some("select"),
                Operation::Count => Some("count()"),
                Operation::First => Some("first()"),
                Operation::Exists => Some("exists()"),
                Operation::Sum(_)
                | Operation::Avg(_)
                | Operation::Min(_)
                | Operation::Max(_)
                | Operation::AggregateExpr { .. }
                | Operation::Explain => Some("an aggregate"),
                Operation::Update(_) => Some("update"),
                Operation::Delete => Some("delete()"),
                Operation::Insert(_) | Operation::InsertMany(_) | Operation::Upsert(_) => {
                    Some("an insert")
                }
                _ => None,
            };
            operations.push(operation);
        }

        self.expect_token(Token::RightBrace)?;
        if result_entry.is_none() {
            return Err(ParseError::InvalidSyntax(
                "A scoped query block must end in a select or a terminal operation such as \
                 count(), first() or update { .. }"
                    .to_string(),
            ));
        }
        if !matches!(self.current_token(), Some(Token::EOF) | None) {
            return Err(ParseError::UnexpectedToken(format!(
                "Expected end of query, found {:?}",
                self.current_token()
            )));
        }

        Ok(OQLQuery {
            collection,
            operations,
        })
    }

    fn substitute_scoped_locals_operation(
        operation: &Operation,
        locals: &std::collections::HashMap<String, Expression>,
    ) -> Operation {
        match operation {
            Operation::Where(block) => Operation::Where(WhereBlock {
                condition: Self::substitute_scoped_locals_condition(&block.condition, locals),
            }),
            Operation::Select(block) => Operation::Select(SelectBlock {
                fields: block
                    .fields
                    .iter()
                    .map(|field| SelectField {
                        alias: field.alias.clone(),
                        expression: Self::substitute_scoped_locals_expression(
                            &field.expression,
                            locals,
                        ),
                    })
                    .collect(),
            }),
            Operation::Insert(block) => Operation::Insert(InsertBlock {
                fields: block
                    .fields
                    .iter()
                    .map(|field| InsertField {
                        name: field.name.clone(),
                        value: Self::substitute_scoped_locals_expression(&field.value, locals),
                    })
                    .collect(),
            }),
            Operation::InsertMany(blocks) => Operation::InsertMany(
                blocks
                    .iter()
                    .map(|block| InsertBlock {
                        fields: block
                            .fields
                            .iter()
                            .map(|field| InsertField {
                                name: field.name.clone(),
                                value: Self::substitute_scoped_locals_expression(
                                    &field.value,
                                    locals,
                                ),
                            })
                            .collect(),
                    })
                    .collect(),
            ),
            Operation::Upsert(block) => Operation::Upsert(UpsertBlock {
                key: block.key.clone(),
                fields: block
                    .fields
                    .iter()
                    .map(|field| InsertField {
                        name: field.name.clone(),
                        value: Self::substitute_scoped_locals_expression(&field.value, locals),
                    })
                    .collect(),
            }),
            Operation::Update(block) => Operation::Update(UpdateBlock {
                fields: block
                    .fields
                    .iter()
                    .map(|field| UpdateField {
                        name: field.name.clone(),
                        value: Self::substitute_scoped_locals_expression(&field.value, locals),
                    })
                    .collect(),
            }),
            Operation::Sort(block) => Operation::Sort(SortBlock {
                fields: block
                    .fields
                    .iter()
                    .map(|field| SortField {
                        expression: Self::substitute_scoped_locals_expression(
                            &field.expression,
                            locals,
                        ),
                        direction: field.direction.clone(),
                    })
                    .collect(),
            }),
            Operation::Having(block) => Operation::Having(HavingBlock {
                condition: Self::substitute_scoped_locals_condition(&block.condition, locals),
            }),
            Operation::LimitExpr(count) => {
                match fold_constant_integer(Self::substitute_scoped_locals_expression(
                    count, locals,
                )) {
                    Expression::Literal(Literal::Integer(count)) if count >= 0 => {
                        Operation::Limit(count as u64)
                    }
                    count => Operation::LimitExpr(count),
                }
            }
            Operation::OffsetExpr(count) => {
                match fold_constant_integer(Self::substitute_scoped_locals_expression(
                    count, locals,
                )) {
                    Expression::Literal(Literal::Integer(count)) if count >= 0 => {
                        Operation::Offset(count as u64)
                    }
                    count => Operation::OffsetExpr(count),
                }
            }
            Operation::AggregateExpr { function, input } => Operation::AggregateExpr {
                function: function.clone(),
                input: Self::substitute_scoped_locals_expression(input, locals),
            },
            Operation::GroupBy(block) => Operation::GroupBy(GroupByBlock {
                fields: block.fields.clone(),
                computed: block
                    .computed
                    .iter()
                    .map(|key| ComputedGroupKey {
                        name: key.name.clone(),
                        expression: Self::substitute_scoped_locals_expression(
                            &key.expression,
                            locals,
                        ),
                    })
                    .collect(),
            }),
            Operation::Nearest(block) => Operation::Nearest(NearestBlock {
                field: block.field.clone(),
                query: Self::substitute_scoped_locals_expression(&block.query, locals),
                metric: block.metric.clone(),
                k: block.k,
                num_candidates: block.num_candidates,
                exact: block.exact,
            }),
            other => other.clone(),
        }
    }

    fn substitute_scoped_locals_condition(
        condition: &Condition,
        locals: &std::collections::HashMap<String, Expression>,
    ) -> Condition {
        match condition {
            Condition::Comparison {
                left,
                operator,
                right,
            } => Condition::Comparison {
                left: Self::substitute_scoped_locals_expression(left, locals),
                operator: operator.clone(),
                right: Self::substitute_scoped_locals_expression(right, locals),
            },
            Condition::And(left, right) => Condition::And(
                Box::new(Self::substitute_scoped_locals_condition(left, locals)),
                Box::new(Self::substitute_scoped_locals_condition(right, locals)),
            ),
            Condition::Or(left, right) => Condition::Or(
                Box::new(Self::substitute_scoped_locals_condition(left, locals)),
                Box::new(Self::substitute_scoped_locals_condition(right, locals)),
            ),
            Condition::Not(inner) => Condition::Not(Box::new(
                Self::substitute_scoped_locals_condition(inner, locals),
            )),
        }
    }

    fn substitute_scoped_locals_expression(
        expression: &Expression,
        locals: &std::collections::HashMap<String, Expression>,
    ) -> Expression {
        match expression {
            Expression::Identifier(name) if locals.contains_key(name) => locals[name].clone(),
            Expression::FieldAccess(field)
                if field.path.len() == 1 && locals.contains_key(&field.path[0]) =>
            {
                locals[&field.path[0]].clone()
            }
            Expression::QueryExpression { query } => {
                if let Some(bound) = locals.get(&query.collection) {
                    let appended_operations = query
                        .operations
                        .iter()
                        .map(|operation| {
                            Self::substitute_scoped_locals_operation(operation, locals)
                        })
                        .collect::<Vec<_>>();
                    match bound {
                        Expression::QueryExpression { query: bound_query } => {
                            let mut lowered = bound_query.clone();
                            lowered.operations.extend(appended_operations);
                            return Expression::QueryExpression { query: lowered };
                        }
                        Expression::CollectionExpression { source, operations } => {
                            let mut lowered = operations.clone();
                            lowered.extend(appended_operations);
                            return Expression::CollectionExpression {
                                source: source.clone(),
                                operations: lowered,
                            };
                        }
                        source => {
                            return Expression::CollectionExpression {
                                source: Box::new(source.clone()),
                                operations: appended_operations,
                            };
                        }
                    }
                }
                Expression::QueryExpression {
                    query: OQLQuery {
                        collection: query.collection.clone(),
                        operations: query
                            .operations
                            .iter()
                            .map(|operation| {
                                Self::substitute_scoped_locals_operation(operation, locals)
                            })
                            .collect(),
                    },
                }
            }
            Expression::CollectionExpression { source, operations } => {
                Expression::CollectionExpression {
                    source: Box::new(Self::substitute_scoped_locals_expression(source, locals)),
                    operations: operations
                        .iter()
                        .map(|operation| {
                            Self::substitute_scoped_locals_operation(operation, locals)
                        })
                        .collect(),
                }
            }
            Expression::FunctionCall { name, args } => Expression::FunctionCall {
                name: name.clone(),
                args: args
                    .iter()
                    .map(|arg| Self::substitute_scoped_locals_expression(arg, locals))
                    .collect(),
            },
            Expression::IfThenElse {
                condition,
                then_expr,
                else_expr,
            } => Expression::IfThenElse {
                condition: Box::new(Self::substitute_scoped_locals_condition(condition, locals)),
                then_expr: Box::new(Self::substitute_scoped_locals_expression(then_expr, locals)),
                else_expr: Box::new(Self::substitute_scoped_locals_expression(else_expr, locals)),
            },
            Expression::Match { expr, arms } => Expression::Match {
                expr: Box::new(Self::substitute_scoped_locals_expression(expr, locals)),
                arms: arms
                    .iter()
                    .map(|arm| MatchArm {
                        pattern: arm.pattern.clone(),
                        guard: arm
                            .guard
                            .as_ref()
                            .map(|guard| Self::substitute_scoped_locals_condition(guard, locals)),
                        body: Self::substitute_scoped_locals_expression(&arm.body, locals),
                    })
                    .collect(),
            },
            Expression::LogicalAnd(left, right) => Expression::LogicalAnd(
                Box::new(Self::substitute_scoped_locals_expression(left, locals)),
                Box::new(Self::substitute_scoped_locals_expression(right, locals)),
            ),
            Expression::LogicalOr(left, right) => Expression::LogicalOr(
                Box::new(Self::substitute_scoped_locals_expression(left, locals)),
                Box::new(Self::substitute_scoped_locals_expression(right, locals)),
            ),
            Expression::LogicalNot(inner) => Expression::LogicalNot(Box::new(
                Self::substitute_scoped_locals_expression(inner, locals),
            )),
            Expression::BinaryOp {
                left,
                operator,
                right,
            } => Expression::BinaryOp {
                left: Box::new(Self::substitute_scoped_locals_expression(left, locals)),
                operator: operator.clone(),
                right: Box::new(Self::substitute_scoped_locals_expression(right, locals)),
            },
            Expression::ArrayAccess { array, index } => Expression::ArrayAccess {
                array: Box::new(Self::substitute_scoped_locals_expression(array, locals)),
                index: Box::new(Self::substitute_scoped_locals_expression(index, locals)),
            },
            Expression::ChainedAccess { base, field } => Expression::ChainedAccess {
                base: Box::new(Self::substitute_scoped_locals_expression(base, locals)),
                field: field.clone(),
            },
            Expression::Block(expressions) => Expression::Block(
                expressions
                    .iter()
                    .map(|expr| Self::substitute_scoped_locals_expression(expr, locals))
                    .collect(),
            ),
            Expression::ArrayLiteral(elements) => Expression::ArrayLiteral(
                elements
                    .iter()
                    .map(|expr| Self::substitute_scoped_locals_expression(expr, locals))
                    .collect(),
            ),
            Expression::ObjectLiteral(fields) => Expression::ObjectLiteral(
                fields
                    .iter()
                    .map(|field| ObjectField {
                        name: field.name.clone(),
                        value: Self::substitute_scoped_locals_expression(&field.value, locals),
                    })
                    .collect(),
            ),
            Expression::ConditionExpr(condition) => Expression::ConditionExpr(Box::new(
                Self::substitute_scoped_locals_condition(condition, locals),
            )),
            Expression::Identifier(_) | Expression::FieldAccess(_) | Expression::Literal(_) => {
                expression.clone()
            }
        }
    }

    /// Parse operation with type validation
    fn parse_operation_with_validation(
        &mut self,
        schema_name: &str,
    ) -> Result<Operation, ParseError> {
        if let Some(operation) = self.parse_computed_operation()? {
            return Ok(operation);
        }
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
                            return Err(ParseError::UnexpectedToken(format!(
                                "Expected ',' or ']', found {:?}",
                                self.current_token()
                            )));
                        }
                    }

                    self.expect_token(Token::RightBracket)?; // consume ']'

                    if insert_blocks.is_empty() {
                        return Err(ParseError::UnexpectedToken(
                            "insertMany requires at least one document".to_string(),
                        ));
                    }

                    Ok(Operation::InsertMany(insert_blocks))
                } else {
                    // Single insert
                    let insert_block = self.parse_insert_block_with_validation(schema_name)?;
                    Ok(Operation::Insert(insert_block))
                }
            }
            Some(Token::Upsert) => {
                self.advance();
                let upsert = self.parse_upsert_block()?;
                if let Some(registry) = &self.schema_registry {
                    if registry
                        .get_field_type(schema_name, &upsert.key)
                        .map_err(|e| ParseError::SchemaError {
                            message: e.to_string(),
                        })?
                        .is_none()
                        && crate::schema::DocumentSystemField::from_name(&upsert.key).is_none()
                    {
                        return Err(ParseError::SchemaError {
                            message: format!(
                                "Upsert key '{}' not found in schema '{}'",
                                upsert.key, schema_name
                            ),
                        });
                    }
                    self.validate_insert_fields(&upsert.fields, schema_name)?;
                }
                Ok(Operation::Upsert(upsert))
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
            Some(Token::Nearest) => {
                self.advance();
                let nearest_block = self.parse_nearest_block()?;
                if self.schema_registry.is_some() {
                    self.validate_field_exists(&nearest_block.field, schema_name)?;
                    if let Some(registry) = &self.schema_registry {
                        let vector_field = nearest_block.field.path.first().ok_or_else(|| {
                            ParseError::InvalidSyntax("nearest.field cannot be empty".to_string())
                        })?;
                        let field_type = registry
                            .get_field_type(schema_name, vector_field)
                            .map_err(|e| ParseError::SchemaError {
                                message: e.to_string(),
                            })?
                            .ok_or_else(|| ParseError::SchemaError {
                                message: format!(
                                    "Field '{}' not found in schema '{}'",
                                    vector_field, schema_name
                                ),
                            })?;
                        match field_type {
                            crate::schema::FieldType::Vector { dim } => {
                                if let Expression::ArrayLiteral(elements) = &nearest_block.query {
                                    if elements.len() != dim as usize {
                                        return Err(ParseError::TypeError {
                                            message: format!(
                                                "nearest.query dimension mismatch for '{}': expected {}, got {}",
                                                vector_field,
                                                dim,
                                                elements.len()
                                            ),
                                        });
                                    }
                                    for element in elements {
                                        match element {
                                            Expression::Literal(Literal::Integer(_))
                                            | Expression::Literal(Literal::Float(_)) => {}
                                            _ => {
                                                return Err(ParseError::TypeError {
                                                    message: "nearest.query must contain numeric literals".to_string(),
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                            other => {
                                return Err(ParseError::TypeError {
                                    message: format!(
                                        "nearest.field '{}' must be Vector type, got {}",
                                        vector_field,
                                        other.type_name()
                                    ),
                                });
                            }
                        }
                    }
                }
                Ok(Operation::Nearest(nearest_block))
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
    fn parse_select_block_with_validation(
        &mut self,
        schema_name: &str,
    ) -> Result<SelectBlock, ParseError> {
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
    fn parse_insert_block_with_validation(
        &mut self,
        schema_name: &str,
    ) -> Result<InsertBlock, ParseError> {
        let insert_block = self.parse_insert_block()?;
        self.validate_insert_fields(&insert_block.fields, schema_name)?;
        Ok(insert_block)
    }

    fn validate_insert_fields(
        &self,
        fields: &[InsertField],
        schema_name: &str,
    ) -> Result<(), ParseError> {
        // Validate field existence and types if schema registry is available
        if let Some(registry) = &self.schema_registry {
            for field in fields {
                // Check field exists in schema
                let field_type = registry
                    .get_field_type(schema_name, &field.name)
                    .map_err(|e| ParseError::SchemaError {
                        message: e.to_string(),
                    })?
                    .ok_or_else(|| ParseError::SchemaError {
                        message: format!(
                            "Field '{}' not found in schema '{}'",
                            field.name, schema_name
                        ),
                    })?;

                // Validate field existence in value expression
                self.validate_expression_fields(&field.value, schema_name)?;

                // Validate value type matches field type
                let value_type = self.infer_expression_type(&field.value, schema_name)?;
                self.validate_type_compatibility(&field_type, &value_type)?;
            }
        }
        Ok(())
    }

    /// Parse update block with type validation
    fn parse_update_block_with_validation(
        &mut self,
        schema_name: &str,
    ) -> Result<UpdateBlock, ParseError> {
        let update_block = self.parse_update_block()?;
        self.validate_update_fields(&update_block.fields, schema_name)?;
        Ok(update_block)
    }

    fn validate_update_fields(
        &self,
        fields: &[UpdateField],
        schema_name: &str,
    ) -> Result<(), ParseError> {
        // Validate field existence and types if schema registry is available
        if let Some(registry) = &self.schema_registry {
            for field in fields {
                let root_name = field.name.split('.').next().unwrap_or(&field.name);
                let field_type = registry
                    .get_field_type(schema_name, root_name)
                    .map_err(|e| ParseError::SchemaError {
                        message: e.to_string(),
                    })?
                    .ok_or_else(|| ParseError::SchemaError {
                        message: format!(
                            "Field '{}' not found in schema '{}'",
                            root_name, schema_name
                        ),
                    })?;

                // Validate field existence in value expression
                self.validate_expression_fields(&field.value, schema_name)?;

                if field.name.contains('.') {
                    let mut container_type = &field_type;
                    while let crate::schema::FieldType::Optional(inner) = container_type {
                        container_type = inner;
                    }
                    if !matches!(container_type, crate::schema::FieldType::Object) {
                        return Err(ParseError::TypeError {
                            message: format!(
                                "Nested update path '{}' must begin at an Object field",
                                field.name
                            ),
                        });
                    }
                } else {
                    let value_type = self.infer_expression_type(&field.value, schema_name)?;
                    self.validate_type_compatibility(&field_type, &value_type)?;
                }
            }
        }
        Ok(())
    }

    fn validate_parsed_operation(
        &self,
        operation: &Operation,
        schema_name: &str,
    ) -> Result<(), ParseError> {
        if self.schema_registry.is_none() {
            return Ok(());
        }
        match operation {
            Operation::Where(block) => {
                self.validate_condition_fields(&block.condition, schema_name)?;
                self.validate_condition_types(&block.condition, schema_name)
            }
            Operation::Select(block) => {
                for field in &block.fields {
                    self.validate_expression_fields(&field.expression, schema_name)?;
                    let _ = self.infer_expression_type(&field.expression, schema_name)?;
                }
                Ok(())
            }
            Operation::Insert(block) => self.validate_insert_fields(&block.fields, schema_name),
            Operation::InsertMany(blocks) => {
                for block in blocks {
                    self.validate_insert_fields(&block.fields, schema_name)?;
                }
                Ok(())
            }
            Operation::Upsert(block) => {
                let registry = self.schema_registry.as_ref().unwrap();
                if registry
                    .get_field_type(schema_name, &block.key)
                    .map_err(|e| ParseError::SchemaError {
                        message: e.to_string(),
                    })?
                    .is_none()
                    && crate::schema::DocumentSystemField::from_name(&block.key).is_none()
                {
                    return Err(ParseError::SchemaError {
                        message: format!(
                            "Upsert key '{}' not found in schema '{}'",
                            block.key, schema_name
                        ),
                    });
                }
                self.validate_insert_fields(&block.fields, schema_name)
            }
            Operation::Update(block) => self.validate_update_fields(&block.fields, schema_name),
            Operation::Sort(block) => {
                for field in &block.fields {
                    self.validate_expression_fields(&field.expression, schema_name)?;
                }
                Ok(())
            }
            Operation::GroupBy(block) => {
                for field in &block.fields {
                    self.validate_field_exists(field, schema_name)?;
                }
                Ok(())
            }
            Operation::Having(block) => {
                self.validate_condition_fields(&block.condition, schema_name)?;
                self.validate_condition_types(&block.condition, schema_name)
            }
            Operation::Sum(field)
            | Operation::Avg(field)
            | Operation::Min(field)
            | Operation::Max(field) => self.validate_field_exists(field, schema_name),
            Operation::Nearest(block) => {
                self.validate_field_exists(&block.field, schema_name)?;
                self.validate_expression_fields(&block.query, schema_name)
            }
            Operation::Delete
            | Operation::Limit(_)
            | Operation::Offset(_)
            | Operation::LimitExpr(_)
            | Operation::OffsetExpr(_)
            | Operation::Distinct
            | Operation::AggregateExpr { .. }
            | Operation::Explain
            | Operation::First
            | Operation::Exists
            | Operation::Count
            | Operation::FunctionDef(_) => Ok(()),
        }
    }

    /// Parse having block with type validation
    fn parse_having_block_with_validation(
        &mut self,
        schema_name: &str,
    ) -> Result<HavingBlock, ParseError> {
        let having_block = self.parse_having_block()?;

        // Validate field existence and types if schema registry is available
        if self.schema_registry.is_some() {
            self.validate_condition_fields(&having_block.condition, schema_name)?;
            self.validate_condition_types(&having_block.condition, schema_name)?;
        }

        Ok(having_block)
    }

    fn parse_operation(&mut self) -> Result<Operation, ParseError> {
        if let Some(operation) = self.parse_computed_operation()? {
            return Ok(operation);
        }
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
            Some(Token::Upsert) => {
                self.advance();
                Ok(Operation::Upsert(self.parse_upsert_block()?))
            }
            Some(Token::Update) => {
                self.advance();
                let update_block = self.parse_update_block()?;
                Ok(Operation::Update(update_block))
            }
            Some(Token::Delete) => {
                self.advance();
                if matches!(self.current_token(), Some(Token::LeftParen)) {
                    self.advance();
                    self.expect_token(Token::RightParen)?;
                }
                Ok(Operation::Delete)
            }
            Some(Token::GroupBy) => {
                self.advance();
                let group_by_block = self.parse_group_by_block()?;
                Ok(Operation::GroupBy(group_by_block))
            }
            Some(Token::Nearest) => {
                self.advance();
                let nearest_block = self.parse_nearest_block()?;
                Ok(Operation::Nearest(nearest_block))
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
                Ok(match self.parse_row_count("limit")? {
                    Expression::Literal(Literal::Integer(count)) => Operation::Limit(count as u64),
                    count => Operation::LimitExpr(count),
                })
            }
            Some(Token::Offset) => {
                self.advance();
                Ok(match self.parse_row_count("offset")? {
                    Expression::Literal(Literal::Integer(count)) => Operation::Offset(count as u64),
                    count => Operation::OffsetExpr(count),
                })
            }
            Some(Token::First) => {
                self.advance();
                self.expect_token(Token::LeftParen)?;
                self.expect_token(Token::RightParen)?;
                Ok(Operation::First)
            }
            Some(Token::Exists) => {
                self.advance();
                self.expect_token(Token::LeftParen)?;
                self.expect_token(Token::RightParen)?;
                Ok(Operation::Exists)
            }
            Some(token) => Err(ParseError::UnexpectedToken(format!(
                "Expected operation method, found {:?}",
                token
            ))),
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
    fn parse_where_block_with_validation(
        &mut self,
        schema_name: &str,
    ) -> Result<WhereBlock, ParseError> {
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
        // A keyword is a field name when it is used as one: as an alias
        // (`min: ..`) or as a bare projected field (`count,`).
        if matches!(
            self.peek_token(),
            Some(Token::Colon | Token::Comma | Token::RightBrace)
        ) {
            self.accept_keyword_as_name();
        }
        // `-name` drops a field that `*` brought in.
        if matches!(self.current_token(), Some(Token::Minus)) {
            let checkpoint = self.position;
            self.advance();
            self.accept_keyword_as_name();
            if let (Some(Token::Identifier(name)), Some(Token::Comma | Token::RightBrace)) =
                (self.current_token(), self.peek_token())
            {
                let name = name.clone();
                self.advance();
                return Ok(SelectField {
                    alias: Some(name),
                    expression: Expression::Identifier(OMITTED_FIELD.to_string()),
                });
            }
            self.position = checkpoint;
        }
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

    fn parse_upsert_block(&mut self) -> Result<UpsertBlock, ParseError> {
        self.expect_token(Token::On)?;
        self.accept_keyword_as_name();
        let key = match self.current_token() {
            Some(Token::Identifier(name)) => {
                let name = name.clone();
                self.advance();
                name
            }
            token => {
                return Err(ParseError::UnexpectedToken(format!(
                    "Expected an upsert key after 'on', found {:?}",
                    token
                )));
            }
        };
        let insert = self.parse_insert_block()?;
        let key_count = insert
            .fields
            .iter()
            .filter(|field| field.name == key)
            .count();
        if key_count != 1 {
            return Err(ParseError::InvalidSyntax(format!(
                "Upsert on '{}' requires exactly one '{}' field in its document",
                key, key
            )));
        }
        Ok(UpsertBlock {
            key,
            fields: insert.fields,
        })
    }

    fn parse_insert_field(&mut self) -> Result<InsertField, ParseError> {
        self.accept_keyword_as_name();
        // Parse field name
        let name = match self.current_token() {
            Some(Token::Identifier(name)) => {
                let field_name = name.clone();
                self.advance();
                field_name
            }
            Some(Token::StringLiteral(name)) => {
                let field_name = name.clone();
                self.advance();
                field_name
            }
            Some(token) => {
                return Err(ParseError::UnexpectedToken(format!(
                    "Expected field name, found {:?}",
                    token
                )))
            }
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
        self.accept_keyword_as_name();
        let first_name = match self.current_token() {
            Some(Token::Identifier(name)) => {
                let field_name = name.clone();
                self.advance();
                field_name
            }
            Some(Token::StringLiteral(name)) => {
                let field_name = name.clone();
                self.advance();
                field_name
            }
            Some(token) => {
                return Err(ParseError::UnexpectedToken(format!(
                    "Expected field name, found {:?}",
                    token
                )))
            }
            None => return Err(ParseError::UnexpectedEndOfInput),
        };

        let mut path = vec![first_name];
        loop {
            match self.current_token() {
                Some(Token::FieldAccess(segments)) => {
                    path.extend(segments.clone());
                    self.advance();
                }
                Some(Token::Dot) => {
                    self.advance();
                    self.accept_keyword_as_name();
                    match self.current_token() {
                        Some(Token::Identifier(name)) => {
                            path.push(name.clone());
                            self.advance();
                        }
                        token => {
                            return Err(ParseError::UnexpectedToken(format!(
                                "Expected field name after '.' in update path, found {:?}",
                                token
                            )));
                        }
                    }
                }
                _ => break,
            }
        }

        let arithmetic_operator = match self.current_token() {
            Some(Token::Colon) => {
                self.advance();
                None
            }
            Some(Token::Plus) => {
                self.advance();
                self.expect_token(Token::Assign)?;
                Some(BinaryOperator::Add)
            }
            Some(Token::Minus) => {
                self.advance();
                self.expect_token(Token::Assign)?;
                Some(BinaryOperator::Subtract)
            }
            Some(Token::Star) => {
                self.advance();
                self.expect_token(Token::Assign)?;
                Some(BinaryOperator::Multiply)
            }
            Some(Token::Slash) => {
                self.advance();
                self.expect_token(Token::Assign)?;
                Some(BinaryOperator::Divide)
            }
            token => {
                return Err(ParseError::UnexpectedToken(format!(
                    "Expected ':', '+=', '-=', '*=', or '/=' after update path, found {:?}",
                    token
                )));
            }
        };

        let right = self.parse_expression()?;
        let value = if let Some(operator) = arithmetic_operator {
            Expression::BinaryOp {
                left: Box::new(Expression::FieldAccess(FieldAccess { path: path.clone() })),
                operator,
                right: Box::new(right),
            }
        } else {
            right
        };

        Ok(UpdateField {
            name: path.join("."),
            value,
        })
    }

    /// `group_by { .status, .user.id, month: format_date(.placed, "%Y-%m") }`.
    ///
    /// A key is a path, or any expression given a name. A bare function
    /// call is named after the function, so `group_by { year(.placed) }`
    /// produces a `year` field.
    fn parse_group_by_block(&mut self) -> Result<GroupByBlock, ParseError> {
        self.expect_token(Token::LeftBrace)?;

        let mut fields = Vec::new();
        let mut computed = Vec::new();
        loop {
            if matches!(self.peek_token(), Some(Token::Colon)) {
                self.accept_keyword_as_name();
            }
            let name = match (self.current_token(), self.peek_token()) {
                (Some(Token::Identifier(name)), Some(Token::Colon)) => {
                    let name = name.clone();
                    self.advance();
                    self.advance();
                    Some(name)
                }
                _ => None,
            };
            match (name, self.parse_expression()?) {
                (None, Expression::FieldAccess(field)) => fields.push(field),
                (None, Expression::Identifier(field)) => {
                    fields.push(FieldAccess { path: vec![field] })
                }
                (None, Expression::FunctionCall { name, args }) => {
                    computed.push(ComputedGroupKey {
                        name: name.clone(),
                        expression: Expression::FunctionCall { name, args },
                    })
                }
                (Some(name), expression) => computed.push(ComputedGroupKey { name, expression }),
                (None, _) => {
                    return Err(ParseError::InvalidSyntax(
                        "a computed group_by key needs a name, as in `group_by { total: .a + .b }`"
                            .to_string(),
                    ))
                }
            }
            if !matches!(self.current_token(), Some(Token::Comma)) {
                break;
            }
            self.advance();
        }

        self.expect_token(Token::RightBrace)?;

        let mut names = std::collections::HashSet::new();
        for name in fields
            .iter()
            .map(|field| field.path.join("_"))
            .chain(computed.iter().map(|key| key.name.clone()))
        {
            if !names.insert(name.clone()) {
                return Err(ParseError::InvalidSyntax(format!(
                    "group_by has two keys named '{name}'"
                )));
            }
        }

        Ok(GroupByBlock { fields, computed })
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

    fn parse_nearest_block(&mut self) -> Result<NearestBlock, ParseError> {
        self.expect_token(Token::LeftBrace)?;

        let mut field: Option<FieldAccess> = None;
        let mut query: Option<Expression> = None;
        let mut metric = VectorMetric::Cosine;
        let mut k: u64 = 10;
        let mut num_candidates: Option<u64> = None;
        let mut exact: Option<bool> = None;

        while !matches!(self.current_token(), Some(Token::RightBrace)) {
            let key = match self.current_token() {
                Some(Token::Identifier(name)) => {
                    let name = name.clone();
                    self.advance();
                    name
                }
                Some(Token::StringLiteral(name)) => {
                    let name = name.clone();
                    self.advance();
                    name
                }
                Some(token) => {
                    return Err(ParseError::UnexpectedToken(format!(
                        "Expected nearest option name, found {:?}",
                        token
                    )))
                }
                None => return Err(ParseError::UnexpectedEndOfInput),
            };

            self.expect_token(Token::Colon)?;

            match key.as_str() {
                "field" => {
                    let expr = self.parse_expression()?;
                    let parsed = match expr {
                        Expression::FieldAccess(path) => path,
                        _ => {
                            return Err(ParseError::TypeError {
                                message: "nearest.field must be a field access (e.g. .embedding)"
                                    .to_string(),
                            })
                        }
                    };
                    field = Some(parsed);
                }
                "query" => {
                    query = Some(self.parse_expression()?);
                }
                "metric" => {
                    let value = self.parse_expression()?;
                    metric = match value {
                        Expression::Literal(Literal::String(s)) => match s.as_str() {
                            "cosine" => VectorMetric::Cosine,
                            "l2" | "euclidean" => VectorMetric::L2,
                            "dot" | "dot_product" => VectorMetric::Dot,
                            _ => {
                                return Err(ParseError::InvalidSyntax(format!(
                                    "Unsupported nearest metric '{}'. Use cosine, l2, or dot",
                                    s
                                )))
                            }
                        },
                        _ => {
                            return Err(ParseError::TypeError {
                                message: "nearest.metric must be a string literal".to_string(),
                            })
                        }
                    };
                }
                "k" => {
                    let value = self.parse_expression()?;
                    k = match value {
                        Expression::Literal(Literal::Integer(v)) if v > 0 => v as u64,
                        _ => {
                            return Err(ParseError::TypeError {
                                message: "nearest.k must be a positive integer".to_string(),
                            })
                        }
                    };
                }
                "num_candidates" => {
                    let value = self.parse_expression()?;
                    num_candidates = match value {
                        Expression::Literal(Literal::Integer(v)) if v > 0 => Some(v as u64),
                        _ => {
                            return Err(ParseError::TypeError {
                                message: "nearest.num_candidates must be a positive integer"
                                    .to_string(),
                            })
                        }
                    };
                }
                "exact" => {
                    let value = self.parse_expression()?;
                    exact = match value {
                        Expression::Literal(Literal::Boolean(v)) => Some(v),
                        _ => {
                            return Err(ParseError::TypeError {
                                message: "nearest.exact must be a boolean".to_string(),
                            })
                        }
                    };
                }
                other => {
                    return Err(ParseError::InvalidSyntax(format!(
                        "Unknown nearest option '{}'",
                        other
                    )));
                }
            }

            if matches!(self.current_token(), Some(Token::Comma)) {
                self.advance();
            } else if !matches!(self.current_token(), Some(Token::RightBrace)) {
                return Err(ParseError::UnexpectedToken(format!(
                    "Expected ',' or '}}', found {:?}",
                    self.current_token()
                )));
            }
        }

        self.expect_token(Token::RightBrace)?;

        let field = field
            .ok_or_else(|| ParseError::InvalidSyntax("nearest requires 'field'".to_string()))?;
        let query = query
            .ok_or_else(|| ParseError::InvalidSyntax("nearest requires 'query'".to_string()))?;

        Ok(NearestBlock {
            field,
            query,
            metric,
            k,
            num_candidates,
            exact,
        })
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
            Some(token) => Err(ParseError::UnexpectedToken(format!(
                "Expected field access, found {:?}",
                token
            ))),
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
        let mut expression = self.parse_expression()?;
        let mut direction = SortDirection::Asc;

        // The tokenizer represents `.created_at.desc` as one field-access
        // token. Treat the final path component as the documented direction
        // modifier instead of accidentally sorting by `created_at.desc`.
        if let Expression::FieldAccess(field) = &mut expression {
            if field.path.len() > 1 {
                match field.path.last().map(String::as_str) {
                    Some("asc") => {
                        field.path.pop();
                        direction = SortDirection::Asc;
                    }
                    Some("desc") => {
                        field.path.pop();
                        direction = SortDirection::Desc;
                    }
                    _ => {}
                }
            }
        }

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
        } else if let Some(Token::Identifier(dir_name)) = self.current_token() {
            // Also accept the readable `.created_at desc` spelling.
            match dir_name.as_str() {
                "asc" => {
                    self.advance();
                    direction = SortDirection::Asc;
                }
                "desc" => {
                    self.advance();
                    direction = SortDirection::Desc;
                }
                _ => {}
            }
        }

        Ok(SortField {
            expression,
            direction,
        })
    }

    /// The row count of `limit(..)` or `offset(..)`: any expression, most
    /// often a literal or a `$parameter`. The bare form `limit 5` remains
    /// for a literal count. A literal is checked here; anything else is
    /// checked when its value is known.
    fn parse_row_count(&mut self, operation: &str) -> Result<Expression, ParseError> {
        let count = if matches!(self.current_token(), Some(Token::LeftParen)) {
            self.advance();
            let count = self.parse_expression()?;
            self.expect_token(Token::RightParen)?;
            fold_constant_integer(count)
        } else {
            Expression::Literal(self.parse_literal()?)
        };
        match &count {
            Expression::Literal(Literal::Integer(value)) if *value >= 0 => Ok(count),
            Expression::Literal(_) => Err(ParseError::InvalidSyntax(format!(
                "{operation} takes a non-negative whole number"
            ))),
            _ => Ok(count),
        }
    }

    fn parse_expression(&mut self) -> Result<Expression, ParseError> {
        self.enter_nested()?;
        let expression = self.parse_logical_or_expression();
        self.depth -= 1;
        expression
    }

    fn enter_nested(&mut self) -> Result<(), ParseError> {
        if self.depth >= MAX_NESTING_DEPTH {
            return Err(ParseError::InvalidSyntax(format!(
                "the query is nested more than {MAX_NESTING_DEPTH} levels deep"
            )));
        }
        self.depth += 1;
        Ok(())
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
            self.parse_comparison_expression()
        }
    }

    fn comparison_operator(token: Option<&Token>) -> Option<ComparisonOperator> {
        Some(match token? {
            Token::Equal => ComparisonOperator::Equal,
            Token::NotEqual => ComparisonOperator::NotEqual,
            Token::LessThan => ComparisonOperator::LessThan,
            Token::GreaterThan => ComparisonOperator::GreaterThan,
            Token::LessEqual => ComparisonOperator::LessEqual,
            Token::GreaterEqual => ComparisonOperator::GreaterEqual,
            Token::Contains => ComparisonOperator::Contains,
            Token::StartsWith => ComparisonOperator::StartsWith,
            Token::EndsWith => ComparisonOperator::EndsWith,
            Token::In => ComparisonOperator::In,
            _ => return None,
        })
    }

    /// A comparison is an ordinary boolean-valued expression, so it can be
    /// projected (`select { adult: .age >= 18 }`), passed to a function, or
    /// stored in a binding, not only used inside `where`.
    fn parse_comparison_expression(&mut self) -> Result<Expression, ParseError> {
        let left = self.parse_additive_expression()?;
        let Some(operator) = Self::comparison_operator(self.current_token()) else {
            return Ok(left);
        };
        self.advance();
        let right = self.parse_additive_expression()?;
        Ok(Expression::ConditionExpr(Box::new(Condition::Comparison {
            left,
            operator,
            right,
        })))
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
            Some(Token::Minus) => {
                // Unary minus binds tighter than any binary operator. A
                // negated numeric literal is folded so `-5` is the literal
                // negative five everywhere a literal is expected.
                self.advance();
                Ok(match self.parse_conditional_expression()? {
                    Expression::Literal(Literal::Integer(value)) => {
                        Expression::Literal(Literal::Integer(value.checked_neg().ok_or_else(
                            || ParseError::InvalidSyntax("integer literal out of range".into()),
                        )?))
                    }
                    Expression::Literal(Literal::Float(value)) => {
                        Expression::Literal(Literal::Float(-value))
                    }
                    operand => Expression::BinaryOp {
                        left: Box::new(Expression::Literal(Literal::Integer(0))),
                        operator: BinaryOperator::Subtract,
                        right: Box::new(operand),
                    },
                })
            }
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
                    if self.is_operation_token(self.current_token()) {
                        let operation = self.parse_operation()?;
                        expr = match expr {
                            Expression::Identifier(collection) => Expression::QueryExpression {
                                query: OQLQuery {
                                    collection,
                                    operations: vec![operation],
                                },
                            },
                            Expression::QueryExpression { mut query } => {
                                query.operations.push(operation);
                                Expression::QueryExpression { query }
                            }
                            Expression::CollectionExpression {
                                source,
                                mut operations,
                            } => {
                                operations.push(operation);
                                Expression::CollectionExpression { source, operations }
                            }
                            source => Expression::CollectionExpression {
                                source: Box::new(source),
                                operations: vec![operation],
                            },
                        };
                    } else if let Some(Token::Identifier(field_name)) = self.current_token() {
                        let field_name = field_name.clone();
                        self.advance();

                        // Method-call style chaining: expr.method(args...)
                        if matches!(self.current_token(), Some(Token::LeftParen)) {
                            self.advance(); // consume '('
                            let mut args = vec![expr];
                            if !matches!(self.current_token(), Some(Token::RightParen)) {
                                let arg = self.parse_expression()?;
                                args.push(arg);
                                while matches!(self.current_token(), Some(Token::Comma)) {
                                    self.advance(); // consume comma
                                    args.push(self.parse_expression()?);
                                }
                            }
                            self.expect_token(Token::RightParen)?;
                            expr = Expression::FunctionCall {
                                name: field_name,
                                args,
                            };
                        } else {
                            // Field/property chaining: expr.field
                            expr = Expression::ChainedAccess {
                                base: Box::new(expr),
                                field: field_name,
                            };
                        }
                    } else {
                        return Err(ParseError::UnexpectedToken(
                            "Expected field name or operation after dot".to_string(),
                        ));
                    }
                }
                Token::FieldAccess(path) => {
                    // Handle field access tokens as chained access
                    // This happens when the tokenizer parsed .field as a FieldAccess token
                    if path.len() == 1 {
                        let field_name = path[0].clone();
                        self.advance();

                        // Method-call style chaining after FieldAccess token: expr.field(args...)
                        if matches!(self.current_token(), Some(Token::LeftParen)) {
                            self.advance(); // consume '('
                            let mut args = vec![expr];
                            if !matches!(self.current_token(), Some(Token::RightParen)) {
                                let arg = self.parse_expression()?;
                                args.push(arg);
                                while matches!(self.current_token(), Some(Token::Comma)) {
                                    self.advance(); // consume comma
                                    args.push(self.parse_expression()?);
                                }
                            }
                            self.expect_token(Token::RightParen)?;
                            expr = Expression::FunctionCall {
                                name: field_name,
                                args,
                            };
                        } else {
                            expr = Expression::ChainedAccess {
                                base: Box::new(expr),
                                field: field_name,
                            };
                        }
                    } else {
                        // Multiple field access - chain them
                        let mut path = path.clone();
                        self.advance();
                        let method = matches!(self.current_token(), Some(Token::LeftParen))
                            .then(|| path.pop().unwrap_or_default());
                        for field_name in path {
                            expr = Expression::ChainedAccess {
                                base: Box::new(expr),
                                field: field_name,
                            };
                        }
                        if let Some(method) = method {
                            expr = self.parse_method_arguments(method, expr)?;
                        }
                    }
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn is_operation_token(&self, token: Option<&Token>) -> bool {
        token.is_some_and(Token::is_operation_keyword)
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
            Some(token) => Err(ParseError::UnexpectedToken(format!(
                "Expected 'then' or '{{' after if condition, found {:?}",
                token
            ))),
            None => Err(ParseError::UnexpectedEndOfInput),
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

        Ok(MatchArm {
            pattern,
            guard,
            body,
        })
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
                let mut path = path.clone();
                self.advance();
                // `.name.upper()` is `upper(.name)`: after a path, a name
                // followed by `(` is a call on what precedes it.
                if matches!(self.current_token(), Some(Token::LeftParen)) {
                    let method = path.pop().unwrap_or_default();
                    let receiver = Expression::FieldAccess(FieldAccess { path });
                    return self.parse_method_arguments(method, receiver);
                }
                Ok(Expression::FieldAccess(FieldAccess { path }))
            }
            // A lone `.` is the current row or value itself, as in
            // `.tags.where { . == "beta" }`.
            Some(Token::Dot)
                if !self
                    .peek_token()
                    .is_some_and(|next| next.is_operation_keyword()) =>
            {
                self.advance();
                Ok(Expression::FieldAccess(FieldAccess { path: Vec::new() }))
            }
            // An operation with no receiver applies to the collection the
            // expression is evaluated over: the rows of the group in a
            // grouped select, or all rows in an aggregate select. So
            // `big: where { .amount > 100 }.count()` counts within the group.
            Some(
                Token::Where
                | Token::Sort
                | Token::Limit
                | Token::Offset
                | Token::Select
                | Token::First
                | Token::Distinct
                | Token::GroupBy,
            ) if matches!(
                self.peek_token(),
                Some(Token::LeftBrace | Token::LeftParen)
            ) =>
            {
                let operation = self.parse_operation()?;
                Ok(Expression::CollectionExpression {
                    source: Box::new(Expression::Identifier(CURRENT_ROWS.to_string())),
                    operations: vec![operation],
                })
            }
            // A keyword where a value is expected, not applied as a
            // function, is a bare field name like any other.
            Some(token)
                if token.is_operation_keyword()
                    && !matches!(self.peek_token(), Some(Token::LeftParen)) =>
            {
                let name = token.keyword_text().unwrap_or_default().to_string();
                self.advance();
                Ok(Expression::Identifier(name))
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
                    Ok(Expression::Identifier(name))
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
            Some(Token::Exists) => {
                self.advance();
                self.parse_function_call("exists".to_string())
            }
            Some(Token::Star) => {
                // Handle * as a special identifier for count(*)
                self.advance();
                Ok(Expression::FieldAccess(FieldAccess {
                    path: vec!["*".to_string()],
                }))
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

    /// Operations whose form is the same with or without schema validation:
    /// `distinct()`, and an aggregate over a computed value.
    fn parse_computed_operation(&mut self) -> Result<Option<Operation>, ParseError> {
        if matches!(self.current_token(), Some(Token::Distinct)) {
            self.advance();
            self.expect_token(Token::LeftParen)?;
            self.expect_token(Token::RightParen)?;
            return Ok(Some(Operation::Distinct));
        }
        if matches!(self.current_token(), Some(Token::Explain)) {
            self.advance();
            self.expect_token(Token::LeftParen)?;
            self.expect_token(Token::RightParen)?;
            return Ok(Some(Operation::Explain));
        }
        let function = match self.current_token() {
            Some(Token::Sum) => "sum",
            Some(Token::Avg) => "avg",
            Some(Token::Min) => "min",
            Some(Token::Max) => "max",
            _ => return Ok(None),
        };
        // A plain path keeps its dedicated operation, which the planner and
        // schema validation understand.
        let simple_path = matches!(
            self.tokens.get(self.position + 2),
            Some(Token::FieldAccess(_) | Token::Identifier(_))
        ) && matches!(
            self.tokens.get(self.position + 3),
            Some(Token::RightParen)
        );
        if simple_path || !matches!(self.peek_token(), Some(Token::LeftParen)) {
            return Ok(None);
        }
        self.advance();
        self.expect_token(Token::LeftParen)?;
        let input = self.parse_expression()?;
        self.expect_token(Token::RightParen)?;
        Ok(Some(Operation::AggregateExpr {
            function: function.to_string(),
            input,
        }))
    }

    /// Parse `(args)` after a method name and build `method(receiver, args)`.
    fn parse_method_arguments(
        &mut self,
        method: String,
        receiver: Expression,
    ) -> Result<Expression, ParseError> {
        self.expect_token(Token::LeftParen)?;
        let mut args = vec![receiver];
        if !matches!(self.current_token(), Some(Token::RightParen)) {
            args.push(self.parse_expression()?);
            while matches!(self.current_token(), Some(Token::Comma)) {
                self.advance();
                args.push(self.parse_expression()?);
            }
        }
        self.expect_token(Token::RightParen)?;
        Ok(Expression::FunctionCall { name: method, args })
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
        self.accept_keyword_as_name();
        // Parse field name
        let name = match self.current_token() {
            Some(Token::Identifier(name)) => {
                let field_name = name.clone();
                self.advance();
                field_name
            }
            Some(Token::StringLiteral(name)) => {
                let field_name = name.clone();
                self.advance();
                field_name
            }
            Some(token) => {
                return Err(ParseError::UnexpectedToken(format!(
                    "Expected field name, found {:?}",
                    token
                )))
            }
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
        self.parse_additive_expression()
    }

    fn parse_condition(&mut self) -> Result<Condition, ParseError> {
        self.enter_nested()?;
        let condition = self.parse_or_condition();
        self.depth -= 1;
        condition
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
        // A leading parenthesis may group a condition, `(.a == 1 || .b == 2)`,
        // or an operand, `(.a + .b) > 5`. Read it as a condition first; if an
        // operator follows the closing parenthesis it was an operand after
        // all, so rewind and read it as one.
        if matches!(self.current_token(), Some(Token::LeftParen)) {
            let checkpoint = self.position;
            self.advance(); // consume '('
            let grouped = self
                .parse_condition()
                .and_then(|condition| self.expect_token(Token::RightParen).map(|_| condition));
            let continues_as_operand = Self::comparison_operator(self.current_token()).is_some()
                || matches!(
                    self.current_token(),
                    Some(
                        Token::Plus
                            | Token::Minus
                            | Token::Star
                            | Token::Slash
                            | Token::Percent
                            | Token::Dot
                            | Token::FieldAccess(_)
                            | Token::LeftBracket
                    )
                );
            match grouped {
                Ok(condition) if !continues_as_operand => return Ok(condition),
                _ => self.position = checkpoint,
            }
        }

        // Parse left side (simple expression without logical operators)
        let left = self.parse_simple_expression()?;

        match Self::comparison_operator(self.current_token()) {
            Some(operator) => {
                self.advance();
                let right = self.parse_simple_expression()?;
                Ok(Condition::Comparison {
                    left,
                    operator,
                    right,
                })
            }
            // No comparison operator: the expression itself is the condition,
            // as in `where { .active }` or `where { is_open(*) }`.
            None => Ok(match left {
                Expression::ConditionExpr(condition) => *condition,
                left => Condition::Comparison {
                    left,
                    operator: ComparisonOperator::Equal,
                    right: Expression::Literal(Literal::Boolean(true)),
                },
            }),
        }
    }

    fn parse_literal(&mut self) -> Result<Literal, ParseError> {
        if matches!(self.current_token(), Some(Token::Minus)) {
            let checkpoint = self.position;
            self.advance();
            return match self.current_token() {
                Some(Token::IntegerLiteral(value)) => {
                    let negated = value.checked_neg().ok_or_else(|| {
                        ParseError::InvalidSyntax("integer literal out of range".into())
                    })?;
                    self.advance();
                    Ok(Literal::Integer(negated))
                }
                Some(Token::FloatLiteral(value)) => {
                    let negated = -*value;
                    self.advance();
                    Ok(Literal::Float(negated))
                }
                _ => {
                    self.position = checkpoint;
                    Err(ParseError::UnexpectedToken(
                        "Expected a number after '-'".to_string(),
                    ))
                }
            };
        }
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
            Some(Token::NullLiteral) => {
                self.advance();
                Ok(Literal::Null)
            }
            Some(token) => Err(ParseError::UnexpectedToken(format!(
                "Expected literal value, found {:?}",
                token
            ))),
            None => Err(ParseError::UnexpectedEndOfInput),
        }
    }

    fn current_token(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }

    fn peek_token(&self) -> Option<&Token> {
        self.tokens.get(self.position + 1)
    }

    /// At a position where the grammar expects a name, read a keyword as
    /// that name. Does nothing for any other token.
    fn accept_keyword_as_name(&mut self) {
        if let Some(text) = self.current_token().and_then(Token::keyword_text) {
            self.tokens[self.position] = Token::Identifier(text.to_string());
        }
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
            Some(token) => Err(ParseError::UnexpectedToken(format!(
                "Expected {:?}, found {:?}",
                expected, token
            ))),
            None => Err(ParseError::UnexpectedEndOfInput),
        }
    }
}

/// Reduce integer arithmetic over literals to one literal, so `limit(10 * 2)`
/// is as plannable as `limit(20)`. Anything else is returned unchanged.
fn fold_constant_integer(expression: Expression) -> Expression {
    fn value(expression: &Expression) -> Option<i64> {
        match expression {
            Expression::Literal(Literal::Integer(value)) => Some(*value),
            Expression::BinaryOp {
                left,
                operator,
                right,
            } => {
                let (left, right) = (value(left)?, value(right)?);
                match operator {
                    BinaryOperator::Add => left.checked_add(right),
                    BinaryOperator::Subtract => left.checked_sub(right),
                    BinaryOperator::Multiply => left.checked_mul(right),
                    BinaryOperator::Divide => left.checked_div(right),
                    BinaryOperator::Modulo => left.checked_rem(right),
                }
            }
            _ => None,
        }
    }
    match value(&expression) {
        Some(folded) => Expression::Literal(Literal::Integer(folded)),
        None => expression,
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

fn expression_uses_legacy_outer(expr: &Expression) -> bool {
    match expr {
        Expression::Identifier(name) => name == "outer",
        Expression::FieldAccess(field_access) => field_access
            .path
            .first()
            .map(|p| p == "outer")
            .unwrap_or(false),
        Expression::ChainedAccess { base, .. } => expression_uses_legacy_outer(base),
        Expression::FunctionCall { args, .. } => args.iter().any(expression_uses_legacy_outer),
        Expression::IfThenElse {
            condition,
            then_expr,
            else_expr,
        } => {
            condition_uses_legacy_outer(condition)
                || expression_uses_legacy_outer(then_expr)
                || expression_uses_legacy_outer(else_expr)
        }
        Expression::Match { expr, arms } => {
            expression_uses_legacy_outer(expr)
                || arms.iter().any(|arm| {
                    arm.guard
                        .as_ref()
                        .map(condition_uses_legacy_outer)
                        .unwrap_or(false)
                        || expression_uses_legacy_outer(&arm.body)
                })
        }
        Expression::LogicalAnd(left, right)
        | Expression::LogicalOr(left, right)
        | Expression::BinaryOp { left, right, .. } => {
            expression_uses_legacy_outer(left) || expression_uses_legacy_outer(right)
        }
        Expression::LogicalNot(inner) => expression_uses_legacy_outer(inner),
        Expression::ArrayAccess { array, index } => {
            expression_uses_legacy_outer(array) || expression_uses_legacy_outer(index)
        }
        Expression::Block(expressions) | Expression::ArrayLiteral(expressions) => {
            expressions.iter().any(expression_uses_legacy_outer)
        }
        Expression::ObjectLiteral(fields) => fields
            .iter()
            .any(|field| expression_uses_legacy_outer(&field.value)),
        Expression::ConditionExpr(condition) => condition_uses_legacy_outer(condition),
        Expression::QueryExpression { query } => query_uses_legacy_outer(query),
        Expression::CollectionExpression { source, operations } => {
            expression_uses_legacy_outer(source)
                || operations.iter().any(operation_uses_legacy_outer)
        }
        Expression::Literal(_) => false,
    }
}

fn condition_uses_legacy_outer(condition: &Condition) -> bool {
    match condition {
        Condition::Comparison { left, right, .. } => {
            expression_uses_legacy_outer(left) || expression_uses_legacy_outer(right)
        }
        Condition::And(left, right) | Condition::Or(left, right) => {
            condition_uses_legacy_outer(left) || condition_uses_legacy_outer(right)
        }
        Condition::Not(inner) => condition_uses_legacy_outer(inner),
    }
}

fn query_uses_legacy_outer(query: &OQLQuery) -> bool {
    if query.collection == "outer" {
        return true;
    }

    query.operations.iter().any(operation_uses_legacy_outer)
}

fn operation_uses_legacy_outer(operation: &Operation) -> bool {
    match operation {
        Operation::Where(block) => condition_uses_legacy_outer(&block.condition),
        Operation::Select(block) => block
            .fields
            .iter()
            .any(|field| expression_uses_legacy_outer(&field.expression)),
        Operation::Insert(block) => block
            .fields
            .iter()
            .any(|field| expression_uses_legacy_outer(&field.value)),
        Operation::InsertMany(blocks) => blocks.iter().any(|block| {
            block
                .fields
                .iter()
                .any(|field| expression_uses_legacy_outer(&field.value))
        }),
        Operation::Upsert(block) => block
            .fields
            .iter()
            .any(|field| expression_uses_legacy_outer(&field.value)),
        Operation::Update(block) => block
            .fields
            .iter()
            .any(|field| expression_uses_legacy_outer(&field.value)),
        Operation::Sort(block) => block
            .fields
            .iter()
            .any(|field| expression_uses_legacy_outer(&field.expression)),
        Operation::Nearest(block) => {
            block
                .field
                .path
                .first()
                .map(|p| p == "outer")
                .unwrap_or(false)
                || expression_uses_legacy_outer(&block.query)
        }
        Operation::GroupBy(block) => block
            .fields
            .iter()
            .any(|field| field.path.first().map(|p| p == "outer").unwrap_or(false)),
        Operation::Having(block) => condition_uses_legacy_outer(&block.condition),
        Operation::Sum(field)
        | Operation::Avg(field)
        | Operation::Min(field)
        | Operation::Max(field) => field.path.first().map(|p| p == "outer").unwrap_or(false),
        Operation::FunctionDef(func) => expression_uses_legacy_outer(&func.body),
        Operation::Delete
        | Operation::Limit(_)
        | Operation::Offset(_)
        | Operation::LimitExpr(_)
        | Operation::OffsetExpr(_)
        | Operation::Distinct
        | Operation::AggregateExpr { .. }
        | Operation::Explain
        | Operation::First
        | Operation::Exists
        | Operation::Count => false,
    }
}

fn reject_legacy_outer(statement: &OQLStatement) -> Result<(), ParseError> {
    let has_legacy_outer = match statement {
        OQLStatement::Query(query) => query_uses_legacy_outer(query),
        OQLStatement::FunctionDef(func) => expression_uses_legacy_outer(&func.body),
    };

    if has_legacy_outer {
        return Err(ParseError::InvalidSyntax(
            "Legacy scope anchor 'outer' is not supported. Use collection-name anchors (e.g. companies.id)."
                .to_string(),
        ));
    }
    Ok(())
}

// Convenience function to parse OQL statements (queries or function definitions)
pub fn parse_oql_statement(input: &str) -> Result<OQLStatement, Box<dyn std::error::Error>> {
    let (tokens, offsets) = Tokenizer::tokenize_with_offsets(input)?;
    let mut parser = Parser::new(tokens);
    let statement = match parser.parse() {
        Ok(statement) => statement,
        Err(error) => return Err(describe_parse_error(input, &offsets, &parser, &error).into()),
    };
    reject_legacy_outer(&statement)?;
    Ok(statement)
}

/// Turn a parse failure into a message about the user's text: what was
/// expected, in the language's own symbols, and where, with the line shown.
fn describe_parse_error(
    input: &str,
    offsets: &[usize],
    parser: &Parser,
    error: &ParseError,
) -> String {
    let offset = offsets
        .get(parser.position)
        .or(offsets.last())
        .copied()
        .unwrap_or(input.len())
        .min(input.len());
    let before = &input[..offset];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
    let column = before[line_start..].chars().count() + 1;
    let text = input[line_start..].lines().next().unwrap_or_default();
    format!(
        "{} at line {line}, column {column}:\n  {}\n  {}^",
        readable_tokens(&error.to_string()),
        text.trim_end(),
        " ".repeat(column - 1)
    )
}

/// Replace the parser's internal token names in a message with the text a
/// user would have typed: `RightBrace` becomes `'}'`.
fn readable_tokens(message: &str) -> String {
    /// Rewrite every `open...close` span using `render` on what is inside.
    fn rewrite(text: &str, open: &str, close: &str, render: impl Fn(&str) -> String) -> String {
        let mut result = String::new();
        let mut rest = text;
        while let Some(start) = rest.find(open) {
            let after = &rest[start + open.len()..];
            let Some(end) = after.find(close) else {
                break;
            };
            result.push_str(&rest[..start]);
            result.push_str(&render(&after[..end]));
            rest = &after[end + close.len()..];
        }
        result.push_str(rest);
        result
    }

    let mut text = rewrite(message, "FieldAccess([", "])", |inner| {
        format!(
            "'.{}'",
            inner
                .split(", ")
                .map(|segment| segment.trim_matches('"'))
                .collect::<Vec<_>>()
                .join(".")
        )
    });
    text = rewrite(&text, "Identifier(\"", "\")", |inner| format!("'{inner}'"));
    text = rewrite(&text, "StringLiteral(\"", "\")", |inner| format!("\"{inner}\""));
    for literal in ["IntegerLiteral(", "FloatLiteral(", "BooleanLiteral("] {
        text = rewrite(&text, literal, ")", |inner| inner.to_string());
    }
    text = rewrite(&text, "Comment(\"", "\")", |_| "a comment".to_string());
    text = rewrite(&text, "Some(", ")", |inner| inner.to_string());
    for (name, shown) in [
        ("LeftBrace", "'{'"),
        ("RightBrace", "'}'"),
        ("LeftParen", "'('"),
        ("RightParen", "')'"),
        ("LeftBracket", "'['"),
        ("RightBracket", "']'"),
        ("GreaterEqual", "'>='"),
        ("LessEqual", "'<='"),
        ("NotEqual", "'!='"),
        ("GreaterThan", "'>'"),
        ("LessThan", "'<'"),
        ("NullLiteral", "null"),
        ("StartsWith", "'starts_with'"),
        ("EndsWith", "'ends_with'"),
        ("GroupBy", "'group_by'"),
        ("Comma", "','"),
        ("Colon", "':'"),
        ("Arrow", "'->'"),
        ("Assign", "'='"),
        ("Equal", "'=='"),
        ("Percent", "'%'"),
        ("Slash", "'/'"),
        ("Minus", "'-'"),
        ("Plus", "'+'"),
        ("Star", "'*'"),
        ("Dot", "'.'"),
        ("EOF", "the end of the query"),
        ("None", "the end of the query"),
    ] {
        text = text.replace(name, shown);
    }
    text
}

// Parse OQL statement with schema registry for validation
pub fn parse_oql_statement_with_schema(
    input: &str,
    schema_registry: crate::schema::SchemaRegistry,
) -> Result<OQLStatement, Box<dyn std::error::Error>> {
    let (tokens, offsets) = Tokenizer::tokenize_with_offsets(input)?;
    let mut parser = Parser::with_schema_registry(tokens, schema_registry);
    let statement = match parser.parse() {
        Ok(statement) => statement,
        Err(error) => return Err(describe_parse_error(input, &offsets, &parser, &error).into()),
    };
    reject_legacy_outer(&statement)?;
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

        assert_eq!(
            tokens,
            vec![
                Token::FieldAccess(vec![
                    "customer".to_string(),
                    "account".to_string(),
                    "secure_token".to_string()
                ]),
                Token::EOF,
            ]
        );
    }

    #[test]
    fn test_tokenize_all_operators() {
        let input = "== != < > <= >= and or not";
        let tokens = Tokenizer::tokenize(input).unwrap();

        assert_eq!(
            tokens,
            vec![
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
            ]
        );
    }

    #[test]
    fn test_tokenize_mixed_literals() {
        let input = r#"42 3.14159 "hello world" true false"#;
        let tokens = Tokenizer::tokenize(input).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::IntegerLiteral(42),
                Token::FloatLiteral(3.14159),
                Token::StringLiteral("hello world".to_string()),
                Token::BooleanLiteral(true),
                Token::BooleanLiteral(false),
                Token::EOF,
            ]
        );
    }

    #[test]
    fn test_tokenize_identifiers_with_underscores() {
        let input = "user_id customer_name _private_field";
        let tokens = Tokenizer::tokenize(input).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Identifier("user_id".to_string()),
                Token::Identifier("customer_name".to_string()),
                Token::Identifier("_private_field".to_string()),
                Token::EOF,
            ]
        );
    }

    #[test]
    fn test_tokenize_keywords_vs_identifiers() {
        // Keywords should be recognized as keywords, not identifiers
        let input = "where whereabouts select selection";
        let tokens = Tokenizer::tokenize(input).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Where,
                Token::Identifier("whereabouts".to_string()),
                Token::Select,
                Token::Identifier("selection".to_string()),
                Token::EOF,
            ]
        );
    }

    #[test]
    fn test_tokenize_empty_string() {
        let input = r#""""#;
        let tokens = Tokenizer::tokenize(input).unwrap();

        assert_eq!(
            tokens,
            vec![Token::StringLiteral("".to_string()), Token::EOF,]
        );
    }

    #[test]
    fn test_tokenize_zero_values() {
        let input = "0 0.0";
        let tokens = Tokenizer::tokenize(input).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::IntegerLiteral(0),
                Token::FloatLiteral(0.0),
                Token::EOF,
            ]
        );
    }

    // ========== AST GENERATION TESTS ==========

    #[test]
    fn test_parse_simple_where() {
        let input = r#"customers.where { .name == "Alice" }"#;
        let query = parse_oql(input).unwrap();

        assert_eq!(query.collection, "customers");
        assert_eq!(query.operations.len(), 1);

        match &query.operations[0] {
            Operation::Where(where_block) => match &where_block.condition {
                Condition::Comparison {
                    left,
                    operator,
                    right,
                } => {
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
            },
            _ => panic!("Expected WHERE operation"),
        }
    }

    #[test]
    fn test_parse_all_comparison_operators() {
        let test_cases = vec![
            (r#"users.where { .age == 25 }"#, ComparisonOperator::Equal),
            (
                r#"users.where { .age != 25 }"#,
                ComparisonOperator::NotEqual,
            ),
            (r#"users.where { .age < 25 }"#, ComparisonOperator::LessThan),
            (
                r#"users.where { .age > 25 }"#,
                ComparisonOperator::GreaterThan,
            ),
            (
                r#"users.where { .age <= 25 }"#,
                ComparisonOperator::LessEqual,
            ),
            (
                r#"users.where { .age >= 25 }"#,
                ComparisonOperator::GreaterEqual,
            ),
        ];

        for (input, expected_op) in test_cases {
            let query = parse_oql(input).unwrap();
            match &query.operations[0] {
                Operation::Where(where_block) => match &where_block.condition {
                    Condition::Comparison { operator, .. } => {
                        assert_eq!(
                            std::mem::discriminant(operator),
                            std::mem::discriminant(&expected_op)
                        );
                    }
                    _ => panic!("Expected comparison condition"),
                },
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
                assert!(
                    matches!(insert_block.fields[0].value, Expression::Literal(Literal::String(ref s)) if s == "Bob")
                );

                assert_eq!(insert_block.fields[1].name, "age");
                assert!(matches!(
                    insert_block.fields[1].value,
                    Expression::Literal(Literal::Integer(30))
                ));
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
                assert!(
                    matches!(insert_block.fields[0].value, Expression::Literal(Literal::String(ref s)) if s == "Alice")
                );

                assert_eq!(insert_block.fields[1].name, "age");
                assert!(matches!(
                    insert_block.fields[1].value,
                    Expression::Literal(Literal::Integer(25))
                ));

                assert_eq!(insert_block.fields[2].name, "score");
                assert!(
                    matches!(insert_block.fields[2].value, Expression::Literal(Literal::Float(f)) if (f - 98.5).abs() < f64::EPSILON)
                );

                assert_eq!(insert_block.fields[3].name, "active");
                assert!(matches!(
                    insert_block.fields[3].value,
                    Expression::Literal(Literal::Boolean(true))
                ));
            }
            _ => panic!("Expected INSERT operation"),
        }
    }

    #[test]
    fn test_parse_insert_with_quoted_field_names() {
        let input =
            r#"users.insert { "id": "u1", "gender": "female", profile: { "city": "Lagos" } }"#;
        let query = parse_oql(input).unwrap();
        match &query.operations[0] {
            Operation::Insert(insert_block) => {
                assert_eq!(insert_block.fields.len(), 3);
                assert_eq!(insert_block.fields[0].name, "id");
                assert_eq!(insert_block.fields[1].name, "gender");
                assert_eq!(insert_block.fields[2].name, "profile");
                match &insert_block.fields[2].value {
                    Expression::ObjectLiteral(fields) => {
                        assert_eq!(fields.len(), 1);
                        assert_eq!(fields[0].name, "city");
                    }
                    _ => panic!("Expected nested object literal"),
                }
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
            Operation::Where(where_block) => match &where_block.condition {
                Condition::Comparison {
                    left,
                    operator,
                    right,
                } => {
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
            },
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
    fn test_parse_where_with_arithmetic_expression() {
        let input = r#"users.where { .age + 5 > 40 }"#;
        let query = parse_oql(input).unwrap();
        assert_eq!(query.operations.len(), 1);
        match &query.operations[0] {
            Operation::Where(where_block) => match &where_block.condition {
                Condition::Comparison {
                    left,
                    operator,
                    right,
                } => {
                    assert!(matches!(operator, ComparisonOperator::GreaterThan));
                    assert!(matches!(left, Expression::BinaryOp { .. }));
                    assert!(matches!(right, Expression::Literal(Literal::Integer(40))));
                }
                _ => panic!("Expected comparison condition"),
            },
            _ => panic!("Expected WHERE operation"),
        }
    }

    #[test]
    fn test_parse_select_with_query_expression_count() {
        let input =
            r#"users.select { id, open_count: findings.where { .status == "open" }.count() }"#;
        let query = parse_oql(input).expect("query parse");
        match &query.operations[0] {
            Operation::Select(block) => {
                assert_eq!(block.fields.len(), 2);
                match &block.fields[1].expression {
                    Expression::QueryExpression { query } => {
                        assert_eq!(query.collection, "findings");
                        assert_eq!(query.operations.len(), 2);
                        assert!(matches!(query.operations[0], Operation::Where(_)));
                        assert!(matches!(query.operations[1], Operation::Count));
                    }
                    other => panic!("expected query expression, got {:?}", other),
                }
            }
            _ => panic!("Expected SELECT operation"),
        }
    }

    #[test]
    fn test_parse_method_style_function_call_on_expression() {
        let input = r#"companies.select { days_in_assessment: (.timeline.assessment.entered_at || .created_at).days_until(.now) }"#;
        let query = parse_oql(input).expect("query parse");
        match &query.operations[0] {
            Operation::Select(block) => match &block.fields[0].expression {
                Expression::FunctionCall { name, args } => {
                    assert_eq!(name, "days_until");
                    assert_eq!(args.len(), 2);
                }
                other => panic!("expected function call, got {:?}", other),
            },
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
            Operation::Where(where_block) => match &where_block.condition {
                Condition::Comparison { left, .. } => match left {
                    Expression::Identifier(name) => assert_eq!(name, "name"),
                    _ => panic!("Expected field access"),
                },
                _ => panic!("Expected comparison condition"),
            },
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
    QueryBinding(QueryBinding),
    ValueBinding(ValueBinding),
    Query(OQLQuery),
    FunctionDef(FunctionDefinition),
    Transaction(Vec<OQLFileStatement>),
    /// `drop collection_name`: remove a collection, its documents, indexes
    /// and schema.
    Drop(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryBinding {
    pub name: String,
    pub query: OQLQuery,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValueBinding {
    pub name: String,
    pub value: Expression,
}

/// Represents a schema definition in OQL syntax
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaDefinition {
    pub name: String,
    pub fields: Vec<SchemaField>,
    /// Facts about the collection as a whole: `unique(a, b)`, `check(..)`,
    /// `index(..)`.
    #[serde(default)]
    pub declarations: Vec<SchemaDeclaration>,
}

/// Represents a field in a schema definition
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SchemaField {
    pub name: String,
    pub field_type: String, // For now, store as string - can be parsed later
    pub optional: bool,
    /// `email: String unique`
    #[serde(default)]
    pub unique: bool,
    /// `plan: String = "free"`: OQL source of the default value.
    #[serde(default)]
    pub default: Option<String>,
    /// `from: Ref(accounts) as outgoing`
    #[serde(default)]
    pub reverse_name: Option<String>,
    /// `owner: Ref(users) on delete cascade`
    #[serde(default)]
    pub on_delete: Option<String>,
    /// `full_name: String from name`: this field replaces `name`.
    #[serde(default)]
    pub renamed_from: Option<String>,
    /// `legacy: removed`: the field is dropped from the collection.
    #[serde(default)]
    pub removed: bool,
}

/// A collection-level declaration inside a schema block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SchemaDeclaration {
    /// `unique(tenant, email)`
    Unique(Vec<String>),
    /// `check(.age >= 0)`: OQL source of the condition.
    Check(String),
    /// `index(.status, .placed)`
    Index(Vec<String>),
    /// `index(.embedding, metric: "cosine")`
    VectorIndex { field: String, metric: String },
}

/// Parse a complete OQL file containing multiple statements
pub fn parse_oql_file(content: &str) -> Result<OQLFile, Box<dyn std::error::Error>> {
    let statements = split_statements_with_lines(content);
    let mut parsed_statements = Vec::new();
    let several = statements.len() > 1;

    for (line, statement) in statements {
        match parse_file_statement(&statement) {
            Ok(Some(parsed)) => parsed_statements.push(parsed),
            Ok(None) => {}
            // In a script, say which statement the problem is in. Positions
            // inside the message count from that statement's first line.
            Err(error) if several => {
                return Err(
                    format!("in the statement that starts on line {line}: {error}").into(),
                )
            }
            Err(error) => return Err(error),
        }
    }

    Ok(OQLFile {
        statements: parsed_statements,
    })
}

fn parse_file_statement(
    statement: &str,
) -> Result<Option<OQLFileStatement>, Box<dyn std::error::Error>> {
    let trimmed = statement.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    Ok(Some(if trimmed.starts_with("schema ") {
        OQLFileStatement::Schema(parse_schema_definition(statement)?)
    } else if let Some(name) = trimmed
        .strip_prefix("drop ")
        .map(str::trim)
        .filter(|name| is_valid_identifier(name))
    {
        OQLFileStatement::Drop(name.to_string())
    } else if trimmed.starts_with("fn ") {
        match parse_oql_statement(statement)? {
            OQLStatement::FunctionDef(func_def) => OQLFileStatement::FunctionDef(func_def),
            _ => return Err("Expected function definition".into()),
        }
    } else if is_transaction_statement_start(trimmed) {
        OQLFileStatement::Transaction(parse_transaction_statement(statement)?)
    } else if let Some(binding) = parse_binding_statement(statement)? {
        binding
    } else {
        OQLFileStatement::Query(parse_oql(statement)?)
    }))
}

/// Split a script into statement source texts.
///
/// The scan tracks strings, comments and bracket depth, so braces inside a
/// string or comment, and blank lines inside a block, never affect where a
/// statement ends. A statement ends at a line break outside all brackets,
/// unless the statement visibly continues: the line ends with an operator or
/// separator (`total =`, `a +`), or the next line does not begin with a name
/// (`.where { .. }`, `{`, `&& ..`).
#[cfg(test)]
fn split_statements(content: &str) -> Vec<String> {
    split_statements_with_lines(content)
        .into_iter()
        .map(|(_, statement)| statement)
        .collect()
}

/// [`split_statements`], with the line each statement starts on.
fn split_statements_with_lines(content: &str) -> Vec<(usize, String)> {
    let line_of = |offset: usize| content[..offset].matches('\n').count() + 1;
    let mut statements = Vec::new();
    let mut start: Option<usize> = None;
    let mut significant_end = 0usize;
    let mut last_significant = ' ';
    let mut depth = 0usize;
    let mut line_ended = false;

    let mut chars = content.char_indices().peekable();
    while let Some((index, character)) = chars.next() {
        if character == '\n' {
            line_ended = true;
            continue;
        }
        if character.is_whitespace() {
            continue;
        }
        if character == '/' && matches!(chars.peek(), Some((_, '/'))) {
            while chars.peek().is_some_and(|(_, next)| *next != '\n') {
                chars.next();
            }
            continue;
        }

        if line_ended && depth == 0 && start.is_some() {
            let continues = !(character.is_ascii_alphabetic() || character == '_')
                || "=,+-*/%&|<>([{.:!".contains(last_significant);
            if !continues {
                let from = start.unwrap();
                statements.push((line_of(from), content[from..significant_end].to_string()));
                start = None;
            }
        }
        line_ended = false;
        start.get_or_insert(index);

        let mut end = index + character.len_utf8();
        match character {
            '"' => {
                // Consume the whole literal so its contents are opaque here.
                while let Some((inner, next)) = chars.next() {
                    end = inner + next.len_utf8();
                    match next {
                        '\\' => {
                            if let Some((escaped, next)) = chars.next() {
                                end = escaped + next.len_utf8();
                            }
                        }
                        '"' => break,
                        _ => {}
                    }
                }
            }
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        significant_end = end;
        last_significant = character;
    }
    if let Some(start) = start {
        statements.push((line_of(start), content[start..significant_end].to_string()));
    }
    statements
}

fn is_transaction_statement_start(trimmed: &str) -> bool {
    let Some(rest) = trimmed.strip_prefix("transaction") else {
        return false;
    };
    rest.is_empty() || rest.starts_with(char::is_whitespace) || rest.starts_with('{')
}

fn parse_transaction_statement(
    statement: &str,
) -> Result<Vec<OQLFileStatement>, Box<dyn std::error::Error>> {
    let trimmed = statement.trim();
    if !is_transaction_statement_start(trimmed) {
        return Err("Invalid transaction statement".into());
    }

    let first_brace = trimmed
        .find('{')
        .ok_or_else(|| "transaction block must start with '{'")?;
    let last_brace = trimmed
        .rfind('}')
        .ok_or_else(|| "transaction block must end with '}'")?;
    if last_brace <= first_brace {
        return Err("invalid transaction block delimiters".into());
    }

    let inner = &trimmed[first_brace + 1..last_brace];
    let parsed = parse_oql_file(inner)?;
    Ok(parsed.statements)
}

fn is_valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn parse_binding_statement(
    statement: &str,
) -> Result<Option<OQLFileStatement>, Box<dyn std::error::Error>> {
    let mut meaningful_lines: Vec<&str> = statement
        .lines()
        .map(|l| l.trim_end())
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with("//")
        })
        .collect();
    if meaningful_lines.is_empty() {
        return Ok(None);
    }

    let first_line = meaningful_lines.remove(0);
    let Some((lhs, first_rhs)) = first_line.split_once('=') else {
        return Ok(None);
    };
    let name = lhs.trim();
    if !is_valid_identifier(name) {
        return Ok(None);
    }

    let mut rhs = first_rhs.trim().to_string();
    for line in meaningful_lines {
        if !rhs.is_empty() {
            rhs.push('\n');
        }
        rhs.push_str(line.trim());
    }
    if rhs.is_empty() {
        return Err(format!("Binding '{}' has empty query expression", name).into());
    }
    if let Ok(query) = parse_oql(&rhs) {
        return Ok(Some(OQLFileStatement::QueryBinding(QueryBinding {
            name: name.to_string(),
            query,
        })));
    }

    let tokens = Tokenizer::tokenize(&rhs)?;
    let mut parser = Parser::new(tokens);
    let value = parser.parse_expression()?;
    if !matches!(parser.current_token(), Some(Token::EOF) | None) {
        return Err(format!(
            "Binding '{}' has trailing tokens after its value expression",
            name
        )
        .into());
    }
    Ok(Some(OQLFileStatement::ValueBinding(ValueBinding {
        name: name.to_string(),
        value,
    })))
}

pub fn resolve_query_with_bindings(
    query: &OQLQuery,
    bindings: &std::collections::HashMap<String, OQLQuery>,
) -> Result<OQLQuery, String> {
    fn resolve_collection(
        name: &str,
        bindings: &std::collections::HashMap<String, OQLQuery>,
        visiting: &mut std::collections::HashSet<String>,
        cache: &mut std::collections::HashMap<String, (String, Vec<Operation>)>,
    ) -> Result<(String, Vec<Operation>), String> {
        if let Some(cached) = cache.get(name) {
            return Ok(cached.clone());
        }
        let Some(bound_query) = bindings.get(name) else {
            return Ok((name.to_string(), Vec::new()));
        };
        if visiting.contains(name) {
            return Err(format!("Query binding cycle detected at '{}'", name));
        }

        visiting.insert(name.to_string());
        let (root_collection, mut inherited_ops) =
            resolve_collection(&bound_query.collection, bindings, visiting, cache)?;
        visiting.remove(name);

        inherited_ops.extend(bound_query.operations.clone());
        let resolved = (root_collection, inherited_ops);
        cache.insert(name.to_string(), resolved.clone());
        Ok(resolved)
    }

    let mut visiting = std::collections::HashSet::new();
    let mut cache: std::collections::HashMap<String, (String, Vec<Operation>)> =
        std::collections::HashMap::new();
    let (collection, mut operations) =
        resolve_collection(&query.collection, bindings, &mut visiting, &mut cache)?;
    for operation in &query.operations {
        operations.push(resolve_operation_bindings(operation, bindings)?);
    }

    Ok(OQLQuery {
        collection,
        operations,
    })
}

pub fn resolve_query_with_value_bindings(
    query: &OQLQuery,
    bindings: &std::collections::HashMap<String, Expression>,
) -> OQLQuery {
    OQLQuery {
        collection: query.collection.clone(),
        operations: query
            .operations
            .iter()
            .map(|operation| Parser::substitute_scoped_locals_operation(operation, bindings))
            .collect(),
    }
}

/// The names of the query parameters (`$name`) a query still expects, without
/// the `$`.
pub fn query_parameters(query: &OQLQuery) -> std::collections::BTreeSet<String> {
    // Parameters are carried as `Identifier("$name")` nodes. Walking the
    // serialized tree finds them wherever the grammar can place an
    // expression, without a second hand-maintained traversal of the AST.
    fn collect(node: &serde_json::Value, found: &mut std::collections::BTreeSet<String>) {
        match node {
            serde_json::Value::Object(fields) => {
                if let Some(serde_json::Value::String(name)) = fields.get("Identifier") {
                    if let Some(parameter) = name.strip_prefix('$') {
                        found.insert(parameter.to_string());
                    }
                }
                fields.values().for_each(|value| collect(value, found));
            }
            serde_json::Value::Array(items) => items.iter().for_each(|item| collect(item, found)),
            _ => {}
        }
    }

    let mut found = std::collections::BTreeSet::new();
    if let Ok(tree) = serde_json::to_value(query) {
        collect(&tree, &mut found);
    }
    found
}

/// Supply values for a query's `$name` parameters.
///
/// This is how a host passes runtime values into a query: the query text is
/// fixed, and values arrive as data, so they can never change the query's
/// structure. Every parameter the query uses must be given a value; extra
/// entries are ignored.
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use owl_db::{bind_parameters, parse_oql, Value};
/// use std::collections::HashMap;
///
/// let query = parse_oql(r#"events.where { .level == $level && .attempts > $min }"#)?;
/// let bound = bind_parameters(
///     &query,
///     &HashMap::from([
///         ("level".to_string(), Value::String("error".into())),
///         ("min".to_string(), Value::Integer(-1)),
///     ]),
/// )?;
/// assert!(owl_db::query_parameters(&bound).is_empty());
/// # Ok(())
/// # }
/// ```
pub fn bind_parameters(
    query: &OQLQuery,
    parameters: &std::collections::HashMap<String, crate::document::Value>,
) -> Result<OQLQuery, String> {
    fn literal(value: &crate::document::Value) -> Result<Expression, String> {
        use crate::document::Value;
        Ok(match value {
            Value::String(text) => Expression::Literal(Literal::String(text.clone())),
            Value::Integer(integer) => Expression::Literal(Literal::Integer(*integer)),
            Value::Float(float) => Expression::Literal(Literal::Float(*float)),
            Value::Boolean(boolean) => Expression::Literal(Literal::Boolean(*boolean)),
            Value::Null => Expression::Literal(Literal::Null),
            // A timestamp travels as its RFC 3339 text, which compares with
            // stored timestamps by instant.
            Value::DateTime(instant) => Expression::Literal(Literal::String(instant.to_rfc3339())),
            Value::List(items) => {
                Expression::ArrayLiteral(items.iter().map(literal).collect::<Result<_, _>>()?)
            }
            Value::Object(fields) => Expression::ObjectLiteral(
                fields
                    .iter()
                    .map(|(name, value)| {
                        Ok(ObjectField {
                            name: name.clone(),
                            value: literal(value)?,
                        })
                    })
                    .collect::<Result<_, String>>()?,
            ),
            Value::Reference(_) | Value::GlobalReference(_) => {
                return Err("a reference cannot be passed as a query parameter".to_string())
            }
        })
    }

    let mut bindings = std::collections::HashMap::new();
    for name in query_parameters(query) {
        let value = parameters
            .get(&name)
            .ok_or_else(|| format!("query parameter ${name} has no value"))?;
        bindings.insert(
            format!("${name}"),
            literal(value).map_err(|error| format!("query parameter ${name}: {error}"))?,
        );
    }
    Ok(resolve_query_with_value_bindings(query, &bindings))
}

pub fn resolve_value_with_bindings(
    value: &Expression,
    bindings: &std::collections::HashMap<String, Expression>,
) -> Expression {
    Parser::substitute_scoped_locals_expression(value, bindings)
}

fn resolve_operation_bindings(
    operation: &Operation,
    bindings: &std::collections::HashMap<String, OQLQuery>,
) -> Result<Operation, String> {
    match operation {
        Operation::Where(block) => Ok(Operation::Where(WhereBlock {
            condition: resolve_condition_bindings(&block.condition, bindings)?,
        })),
        Operation::Select(block) => {
            let mut fields = Vec::with_capacity(block.fields.len());
            for field in &block.fields {
                fields.push(SelectField {
                    alias: field.alias.clone(),
                    expression: resolve_expression_bindings(&field.expression, bindings)?,
                });
            }
            Ok(Operation::Select(SelectBlock { fields }))
        }
        Operation::Insert(block) => {
            let mut fields = Vec::with_capacity(block.fields.len());
            for field in &block.fields {
                fields.push(InsertField {
                    name: field.name.clone(),
                    value: resolve_expression_bindings(&field.value, bindings)?,
                });
            }
            Ok(Operation::Insert(InsertBlock { fields }))
        }
        Operation::InsertMany(blocks) => {
            let mut out = Vec::with_capacity(blocks.len());
            for block in blocks {
                let mut fields = Vec::with_capacity(block.fields.len());
                for field in &block.fields {
                    fields.push(InsertField {
                        name: field.name.clone(),
                        value: resolve_expression_bindings(&field.value, bindings)?,
                    });
                }
                out.push(InsertBlock { fields });
            }
            Ok(Operation::InsertMany(out))
        }
        Operation::Upsert(block) => Ok(Operation::Upsert(UpsertBlock {
            key: block.key.clone(),
            fields: block
                .fields
                .iter()
                .map(|field| {
                    Ok(InsertField {
                        name: field.name.clone(),
                        value: resolve_expression_bindings(&field.value, bindings)?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
        })),
        Operation::Update(block) => {
            let mut fields = Vec::with_capacity(block.fields.len());
            for field in &block.fields {
                fields.push(UpdateField {
                    name: field.name.clone(),
                    value: resolve_expression_bindings(&field.value, bindings)?,
                });
            }
            Ok(Operation::Update(UpdateBlock { fields }))
        }
        Operation::Sort(block) => {
            let mut fields = Vec::with_capacity(block.fields.len());
            for field in &block.fields {
                fields.push(SortField {
                    expression: resolve_expression_bindings(&field.expression, bindings)?,
                    direction: field.direction.clone(),
                });
            }
            Ok(Operation::Sort(SortBlock { fields }))
        }
        Operation::Having(block) => Ok(Operation::Having(HavingBlock {
            condition: resolve_condition_bindings(&block.condition, bindings)?,
        })),
        other => Ok(other.clone()),
    }
}

fn resolve_condition_bindings(
    condition: &Condition,
    bindings: &std::collections::HashMap<String, OQLQuery>,
) -> Result<Condition, String> {
    match condition {
        Condition::Comparison {
            left,
            operator,
            right,
        } => Ok(Condition::Comparison {
            left: resolve_expression_bindings(left, bindings)?,
            operator: operator.clone(),
            right: resolve_expression_bindings(right, bindings)?,
        }),
        Condition::And(left, right) => Ok(Condition::And(
            Box::new(resolve_condition_bindings(left, bindings)?),
            Box::new(resolve_condition_bindings(right, bindings)?),
        )),
        Condition::Or(left, right) => Ok(Condition::Or(
            Box::new(resolve_condition_bindings(left, bindings)?),
            Box::new(resolve_condition_bindings(right, bindings)?),
        )),
        Condition::Not(inner) => Ok(Condition::Not(Box::new(resolve_condition_bindings(
            inner, bindings,
        )?))),
    }
}

fn resolve_expression_bindings(
    expr: &Expression,
    bindings: &std::collections::HashMap<String, OQLQuery>,
) -> Result<Expression, String> {
    match expr {
        // A binding is a name for its expression: using the name is the
        // same as writing the bound query in its place. So a binding to an
        // aggregate is a value, and a binding to a pipeline is a collection.
        Expression::Identifier(name) if bindings.contains_key(name) => {
            Ok(Expression::QueryExpression {
                query: resolve_query_with_bindings(
                    &OQLQuery {
                        collection: name.clone(),
                        operations: Vec::new(),
                    },
                    bindings,
                )?,
            })
        }
        Expression::Identifier(_) | Expression::FieldAccess(_) | Expression::Literal(_) => {
            Ok(expr.clone())
        }
        Expression::FunctionCall { name, args } => {
            let mut resolved_args = Vec::with_capacity(args.len());
            for arg in args {
                resolved_args.push(resolve_expression_bindings(arg, bindings)?);
            }
            Ok(Expression::FunctionCall {
                name: name.clone(),
                args: resolved_args,
            })
        }
        Expression::IfThenElse {
            condition,
            then_expr,
            else_expr,
        } => Ok(Expression::IfThenElse {
            condition: Box::new(resolve_condition_bindings(condition, bindings)?),
            then_expr: Box::new(resolve_expression_bindings(then_expr, bindings)?),
            else_expr: Box::new(resolve_expression_bindings(else_expr, bindings)?),
        }),
        Expression::Match { expr, arms } => {
            let mut resolved_arms = Vec::with_capacity(arms.len());
            for arm in arms {
                resolved_arms.push(MatchArm {
                    pattern: arm.pattern.clone(),
                    guard: arm
                        .guard
                        .as_ref()
                        .map(|g| resolve_condition_bindings(g, bindings))
                        .transpose()?,
                    body: resolve_expression_bindings(&arm.body, bindings)?,
                });
            }
            Ok(Expression::Match {
                expr: Box::new(resolve_expression_bindings(expr, bindings)?),
                arms: resolved_arms,
            })
        }
        Expression::LogicalAnd(left, right) => Ok(Expression::LogicalAnd(
            Box::new(resolve_expression_bindings(left, bindings)?),
            Box::new(resolve_expression_bindings(right, bindings)?),
        )),
        Expression::LogicalOr(left, right) => Ok(Expression::LogicalOr(
            Box::new(resolve_expression_bindings(left, bindings)?),
            Box::new(resolve_expression_bindings(right, bindings)?),
        )),
        Expression::LogicalNot(inner) => Ok(Expression::LogicalNot(Box::new(
            resolve_expression_bindings(inner, bindings)?,
        ))),
        Expression::BinaryOp {
            left,
            operator,
            right,
        } => Ok(Expression::BinaryOp {
            left: Box::new(resolve_expression_bindings(left, bindings)?),
            operator: operator.clone(),
            right: Box::new(resolve_expression_bindings(right, bindings)?),
        }),
        Expression::ArrayAccess { array, index } => Ok(Expression::ArrayAccess {
            array: Box::new(resolve_expression_bindings(array, bindings)?),
            index: Box::new(resolve_expression_bindings(index, bindings)?),
        }),
        Expression::ChainedAccess { base, field } => Ok(Expression::ChainedAccess {
            base: Box::new(resolve_expression_bindings(base, bindings)?),
            field: field.clone(),
        }),
        Expression::Block(expressions) => {
            let mut resolved = Vec::with_capacity(expressions.len());
            for e in expressions {
                resolved.push(resolve_expression_bindings(e, bindings)?);
            }
            Ok(Expression::Block(resolved))
        }
        Expression::ArrayLiteral(elements) => {
            let mut resolved = Vec::with_capacity(elements.len());
            for e in elements {
                resolved.push(resolve_expression_bindings(e, bindings)?);
            }
            Ok(Expression::ArrayLiteral(resolved))
        }
        Expression::ObjectLiteral(fields) => {
            let mut resolved = Vec::with_capacity(fields.len());
            for field in fields {
                resolved.push(ObjectField {
                    name: field.name.clone(),
                    value: resolve_expression_bindings(&field.value, bindings)?,
                });
            }
            Ok(Expression::ObjectLiteral(resolved))
        }
        Expression::ConditionExpr(condition) => Ok(Expression::ConditionExpr(Box::new(
            resolve_condition_bindings(condition, bindings)?,
        ))),
        Expression::QueryExpression { query } => Ok(Expression::QueryExpression {
            query: resolve_query_with_bindings(query, bindings)?,
        }),
        Expression::CollectionExpression { source, operations } => {
            Ok(Expression::CollectionExpression {
                source: Box::new(resolve_expression_bindings(source, bindings)?),
                operations: operations
                    .iter()
                    .map(|operation| resolve_operation_bindings(operation, bindings))
                    .collect::<Result<Vec<_>, _>>()?,
            })
        }
    }
}

/// Parse a schema definition from OQL syntax
/// Split `text` at the given separators, ignoring any that sit inside
/// brackets or a string literal.
fn split_top_level(text: &str, separators: &[char]) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut chars = text.chars();
    while let Some(character) = chars.next() {
        match character {
            '"' => {
                current.push(character);
                while let Some(inner) = chars.next() {
                    current.push(inner);
                    match inner {
                        '\\' => current.extend(chars.next()),
                        '"' => break,
                        _ => {}
                    }
                }
                continue;
            }
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            separator if depth == 0 && separators.contains(&separator) => {
                parts.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(character);
    }
    parts.push(current);
    parts
}

/// The text between a leading `keyword(` and its closing `)`.
fn declaration_arguments<'a>(entry: &'a str, keyword: &str) -> Option<&'a str> {
    entry
        .strip_prefix(keyword)
        .map(str::trim_start)
        .and_then(|rest| rest.strip_prefix('('))
        .and_then(|rest| rest.trim_end().strip_suffix(')'))
}

/// A field name written as a path (`.email`) or bare (`email`).
fn declared_field_name(text: &str) -> Result<String, String> {
    let name = text.trim().trim_start_matches('.');
    if name.split('.').all(is_valid_identifier) {
        Ok(name.to_string())
    } else {
        Err(format!("'{}' is not a field name", text.trim()))
    }
}

fn parse_schema_definition(
    statement: &str,
) -> Result<SchemaDefinition, Box<dyn std::error::Error>> {
    // Comments carry no meaning; drop them before looking at structure.
    let source: String = statement
        .lines()
        .map(|line| line.split("//").next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");

    let header_end = source
        .find('{')
        .ok_or("schema declaration must have a '{ ... }' body")?;
    let body_end = source
        .rfind('}')
        .filter(|end| *end > header_end)
        .ok_or("schema declaration is missing its closing '}'")?;
    let mut header = source[..header_end].split_whitespace();
    if header.next() != Some("schema") {
        return Err("Invalid schema definition".into());
    }
    let schema_name = header.next().ok_or("schema declaration has no name")?;
    if !is_valid_identifier(schema_name) || header.next().is_some() {
        return Err(format!("'{}' is not a valid schema name", source[..header_end].trim()).into());
    }

    // Entries are separated by commas or line breaks outside brackets, so a
    // schema reads the same on one line or many.
    let mut fields = Vec::new();
    let mut declarations = Vec::new();
    for entry in split_top_level(&source[header_end + 1..body_end], &[',', '\n']) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let fail = |problem: String| format!("schema '{schema_name}': {problem}");

        // Facts about the collection as a whole.
        if let Some(arguments) = declaration_arguments(entry, "unique") {
            let names = split_top_level(arguments, &[','])
                .iter()
                .map(|name| declared_field_name(name))
                .collect::<Result<Vec<_>, _>>()
                .map_err(fail)?;
            declarations.push(SchemaDeclaration::Unique(names));
            continue;
        }
        if let Some(condition) = declaration_arguments(entry, "check") {
            parse_check_condition(condition).map_err(|error| fail(error.to_string()))?;
            declarations.push(SchemaDeclaration::Check(condition.trim().to_string()));
            continue;
        }
        if let Some(arguments) = declaration_arguments(entry, "index") {
            let mut names = Vec::new();
            let mut metric = None;
            for argument in split_top_level(arguments, &[',']) {
                match argument.split_once(':') {
                    Some((option, value)) if option.trim() == "metric" => {
                        metric = Some(value.trim().trim_matches('"').to_string());
                    }
                    Some((option, _)) => {
                        return Err(fail(format!("unknown index option '{}'", option.trim())).into())
                    }
                    None => names.push(declared_field_name(&argument).map_err(fail)?),
                }
            }
            declarations.push(match (metric, names.as_slice()) {
                (Some(metric), [field]) => SchemaDeclaration::VectorIndex {
                    field: field.clone(),
                    metric,
                },
                (Some(_), _) => {
                    return Err(fail("a vector index covers exactly one field".to_string()).into())
                }
                (None, []) => return Err(fail("index() needs a field".to_string()).into()),
                (None, _) => SchemaDeclaration::Index(names),
            });
            continue;
        }

        let (name, declared) = entry
            .split_once(':')
            .ok_or_else(|| fail(format!("expected 'name: Type', found '{entry}'")))?;
        let name = name.trim().trim_matches('"');
        if name.is_empty() || declared.trim().is_empty() {
            return Err(fail(format!("incomplete field '{entry}'")).into());
        }
        fields.push(parse_schema_field(name, declared).map_err(fail)?);
    }

    Ok(SchemaDefinition {
        name: schema_name.to_string(),
        fields,
        declarations,
    })
}

/// Parse what follows `name:` in a schema block:
/// `Type [as reverse] [on delete rule] [from old] [unique] [= default]`.
fn parse_schema_field(name: &str, declared: &str) -> Result<SchemaField, String> {
    let mut field = SchemaField {
        name: name.to_string(),
        ..SchemaField::default()
    };
    // The default is the rest of the entry, so it may contain anything.
    let mut halves = split_top_level(declared, &['=']).into_iter();
    let before_default = halves.next().unwrap_or_default();
    let default: Vec<String> = halves.collect();
    if !default.is_empty() {
        let source = default.join("=").trim().to_string();
        let tokens = Tokenizer::tokenize(&source).map_err(|error| error.to_string())?;
        let mut parser = Parser::new(tokens);
        parser
            .parse_expression()
            .map_err(|error| format!("default of '{name}': {error}"))?;
        if !matches!(parser.current_token(), Some(Token::EOF) | None) {
            return Err(format!("default of '{name}' has text after its value"));
        }
        field.default = Some(source);
    }

    let mut words = split_top_level(before_default.trim(), &[' ', '\t', '\n'])
        .into_iter()
        .filter(|word| !word.trim().is_empty());
    let declared_type = words.next().ok_or_else(|| format!("field '{name}' has no type"))?;
    if declared_type == "removed" {
        field.removed = true;
    }
    // A trailing `?` is shorthand for an optional field.
    match declared_type.strip_suffix('?') {
        Some(inner) => {
            field.field_type = inner.trim().to_string();
            field.optional = true;
        }
        None => field.field_type = declared_type,
    }
    while let Some(word) = words.next() {
        let mut argument = |what: &str| {
            words
                .next()
                .filter(|value| is_valid_identifier(value))
                .ok_or_else(|| format!("field '{name}': expected {what} after '{word}'"))
        };
        match word.as_str() {
            "unique" => field.unique = true,
            "as" => field.reverse_name = Some(argument("a name")?),
            "from" => field.renamed_from = Some(argument("a field name")?),
            "on" => {
                if argument("'delete'")? != "delete" {
                    return Err(format!("field '{name}': expected 'on delete <rule>'"));
                }
                let rule = argument("restrict, cascade or nullify")?;
                if !matches!(rule.as_str(), "restrict" | "cascade" | "nullify") {
                    return Err(format!(
                        "field '{name}': '{rule}' is not a delete rule; use restrict, cascade or nullify"
                    ));
                }
                field.on_delete = Some(rule);
            }
            other => {
                return Err(format!("field '{name}': unexpected '{other}' after the type"));
            }
        }
    }
    Ok(field)
}

/// Parse the condition of a `check(..)` declaration.
pub fn parse_check_condition(source: &str) -> Result<Condition, Box<dyn std::error::Error>> {
    let tokens = Tokenizer::tokenize(source)?;
    let mut parser = Parser::new(tokens);
    let condition = parser.parse_condition()?;
    if !matches!(parser.current_token(), Some(Token::EOF) | None) {
        return Err("check() has text after its condition".into());
    }
    Ok(condition)
}

/// Parse the OQL source of a stored default value or check into an
/// expression.
pub fn parse_stored_expression(source: &str) -> Result<Expression, Box<dyn std::error::Error>> {
    let tokens = Tokenizer::tokenize(source)?;
    Ok(Parser::new(tokens).parse_expression()?)
}

/// Execute a parsed OQL file
pub fn execute_oql_file<S: crate::storage::DocumentStore>(
    file: &OQLFile,
    executor: &crate::executor::QueryExecutor<S>,
) -> Result<OQLFileExecutionResult, Box<dyn std::error::Error>> {
    let mut script_executor =
        crate::executor::QueryExecutor::new_with_real_clock(executor.store().clone());
    let mut results = Vec::new();
    let mut schema_count = 0;
    let mut query_count = 0;
    let mut function_count = 0;
    let mut binding_count = 0;
    let mut success_count = 0;
    let mut transaction_count = 0;
    let mut errors = Vec::new();
    let mut query_bindings: std::collections::HashMap<String, OQLQuery> =
        std::collections::HashMap::new();
    let mut value_bindings: std::collections::HashMap<String, Expression> =
        std::collections::HashMap::new();

    // Create a schema registry for type resolution
    let registry = crate::schema::SchemaRegistry::new();
    let mut schema_failures: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    // First pass: Register all schema names (for forward references)
    for statement in &file.statements {
        if let OQLFileStatement::Schema(schema_def) = statement {
            // Create empty schema just to register the name
            let empty_schema = crate::schema::Schema::new(
                schema_def.name.clone(),
                std::collections::HashMap::new(),
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
                    // Declare it, or apply the change to an existing one.
                    if let Err(error) = script_executor.define_schema(schema_def, schema) {
                        schema_failures.insert(schema_def.name.clone(), error.to_string());
                    }
                }
                Err(e) => {
                    schema_failures.insert(schema_def.name.clone(), e.to_string());
                }
            }
        }
    }

    // Third pass: Execute statements
    let mut not_run = 0usize;
    for statement in &file.statements {
        // A script is a sequence: once a statement has failed, the ones
        // after it were written for a state that does not exist, so they
        // are not run.
        if !errors.is_empty() {
            not_run += 1;
            continue;
        }
        match statement {
            OQLFileStatement::Schema(schema) => {
                schema_count += 1;
                if let Some(reason) = schema_failures.get(&schema.name) {
                    let message = format!("Schema '{}' was not defined: {}", schema.name, reason);
                    println!("❌ {}", message);
                    errors.push(message.clone());
                    results.push(StatementResult::Error(message));
                    continue;
                }
                println!("📋 Schema definition: {}", schema.name);
                success_count += 1;
                results.push(StatementResult::Schema(schema.name.clone()));
            }
            OQLFileStatement::Drop(name) => {
                query_count += 1;
                match script_executor.drop_collection(name) {
                    Ok(removed) => {
                        println!("🗑️  Dropped {} ({} documents)", name, removed);
                        success_count += 1;
                        results.push(StatementResult::Query(removed));
                    }
                    Err(error) => {
                        let message = format!("Drop failed: {}", error);
                        println!("❌ {}", message);
                        errors.push(message.clone());
                        results.push(StatementResult::Error(message));
                    }
                }
            }
            OQLFileStatement::QueryBinding(binding) => {
                binding_count += 1;
                query_bindings.insert(binding.name.clone(), binding.query.clone());
                println!("🔗 Query binding: {}", binding.name);
                success_count += 1;
                results.push(StatementResult::Binding(binding.name.clone()));
            }
            OQLFileStatement::ValueBinding(binding) => {
                binding_count += 1;
                let value =
                    Parser::substitute_scoped_locals_expression(&binding.value, &value_bindings);
                // A script value is also reachable as `$name`, so a query
                // written with parameters runs unchanged inside a script
                // that defines them.
                value_bindings.insert(format!("${}", binding.name), value.clone());
                value_bindings.insert(binding.name.clone(), value);
                success_count += 1;
                results.push(StatementResult::Binding(binding.name.clone()));
            }
            OQLFileStatement::Query(query) => {
                query_count += 1;
                let start_time = std::time::Instant::now();
                let resolved_query = match resolve_query_with_bindings(query, &query_bindings) {
                    Ok(q) => resolve_query_with_value_bindings(&q, &value_bindings),
                    Err(e) => {
                        let error_msg = format!("Binding resolution failed: {}", e);
                        println!("❌ {}", error_msg);
                        errors.push(error_msg.clone());
                        results.push(StatementResult::Error(error_msg));
                        continue;
                    }
                };
                let execution = if let Some(source) = value_bindings.get(&resolved_query.collection)
                {
                    script_executor
                        .evaluate_script_expression(&Expression::CollectionExpression {
                            source: Box::new(source.clone()),
                            operations: resolved_query.operations.clone(),
                        })
                        .map(|value| match value {
                            crate::document::Value::List(values) => values.len(),
                            crate::document::Value::Null => 0,
                            _ => 1,
                        })
                } else {
                    script_executor
                        .execute_query(&resolved_query)
                        .map(|documents| documents.len())
                };
                match execution {
                    Ok(document_count) => {
                        let elapsed = start_time.elapsed();
                        success_count += 1;
                        let query_str = format!(
                            "{}.{}",
                            resolved_query.collection,
                            resolved_query
                                .operations
                                .iter()
                                .map(|op| format!("{:?}", op))
                                .collect::<Vec<_>>()
                                .join(".")
                        );
                        println!(
                            "✅ {} -> {} documents ({:.3}ms)",
                            query_str.chars().take(50).collect::<String>(),
                            document_count,
                            elapsed.as_secs_f64() * 1000.0
                        );
                        results.push(StatementResult::Query(document_count));
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
                match script_executor.register_user_function(func_def.clone()) {
                    Ok(()) => {
                        println!("🔧 Function definition registered: {}", func_def.name);
                        success_count += 1;
                        results.push(StatementResult::Function(func_def.name.clone()));
                    }
                    Err(e) => {
                        let msg = format!("Function registration failed: {}", e);
                        println!("❌ {}", msg);
                        errors.push(msg.clone());
                        results.push(StatementResult::Error(msg));
                    }
                }
            }
            OQLFileStatement::Transaction(tx_statements) => {
                transaction_count += 1;
                // The storage engine's own transaction makes the block
                // atomic: its writes are published together or not at all.
                // Only a store without that mechanism needs the fallback of
                // copying the database so it can be put back.
                let mut active_txn_id = None;
                let mut snapshot = None;
                if let Some(rust_store) = script_executor
                    .store()
                    .as_any()
                    .downcast_ref::<crate::storage::RustStore>()
                {
                    let tx_id = rust_store.begin_local_transaction()?;
                    active_txn_id = Some(tx_id);
                } else {
                    snapshot = Some(snapshot_store(script_executor.store().as_ref())?);
                }
                let mut tx_functions: Vec<String> = Vec::new();
                let mut tx_errors = Vec::new();
                let mut tx_results = Vec::new();
                let mut tx_bindings = query_bindings.clone();
                let mut tx_value_bindings = value_bindings.clone();
                let mut tx_success = 0usize;

                for tx_stmt in tx_statements {
                    match tx_stmt {
                        OQLFileStatement::Schema(schema) => {
                            schema_count += 1;
                            tx_success += 1;
                            tx_results.push(StatementResult::Schema(schema.name.clone()));
                        }
                        OQLFileStatement::FunctionDef(func_def) => {
                            function_count += 1;
                            match script_executor.register_user_function(func_def.clone()) {
                                Ok(()) => {
                                    tx_success += 1;
                                    tx_functions.push(func_def.name.clone());
                                    tx_results
                                        .push(StatementResult::Function(func_def.name.clone()));
                                }
                                Err(e) => {
                                    tx_errors.push(format!("Function registration failed: {}", e));
                                }
                            }
                        }
                        OQLFileStatement::QueryBinding(binding) => {
                            binding_count += 1;
                            tx_bindings.insert(binding.name.clone(), binding.query.clone());
                            tx_success += 1;
                            tx_results.push(StatementResult::Binding(binding.name.clone()));
                        }
                        OQLFileStatement::ValueBinding(binding) => {
                            binding_count += 1;
                            let value = Parser::substitute_scoped_locals_expression(
                                &binding.value,
                                &tx_value_bindings,
                            );
                            tx_value_bindings.insert(format!("${}", binding.name), value.clone());
                            tx_value_bindings.insert(binding.name.clone(), value);
                            tx_success += 1;
                            tx_results.push(StatementResult::Binding(binding.name.clone()));
                        }
                        OQLFileStatement::Query(query) => {
                            query_count += 1;
                            let resolved_query =
                                match resolve_query_with_bindings(query, &tx_bindings) {
                                    Ok(q) => {
                                        resolve_query_with_value_bindings(&q, &tx_value_bindings)
                                    }
                                    Err(e) => {
                                        tx_errors.push(format!("Binding resolution failed: {}", e));
                                        continue;
                                    }
                                };
                            let execution = if let Some(source) =
                                tx_value_bindings.get(&resolved_query.collection)
                            {
                                script_executor
                                    .evaluate_script_expression(&Expression::CollectionExpression {
                                        source: Box::new(source.clone()),
                                        operations: resolved_query.operations.clone(),
                                    })
                                    .map(|value| match value {
                                        crate::document::Value::List(values) => values.len(),
                                        crate::document::Value::Null => 0,
                                        _ => 1,
                                    })
                            } else {
                                script_executor
                                    .execute_query(&resolved_query)
                                    .map(|documents| documents.len())
                            };
                            match execution {
                                Ok(document_count) => {
                                    tx_success += 1;
                                    tx_results.push(StatementResult::Query(document_count));
                                }
                                Err(e) => {
                                    tx_errors.push(format!("Execution failed: {}", e));
                                }
                            }
                        }
                        OQLFileStatement::Transaction(_) => {
                            tx_errors
                                .push("Nested transaction blocks are not supported".to_string());
                        }
                        OQLFileStatement::Drop(name) => {
                            tx_errors.push(format!(
                                "'drop {name}' cannot run inside a transaction block"
                            ));
                        }
                    }
                }

                // A commit that fails is a failed transaction like any
                // other: nothing was published, and the block reports it.
                if tx_errors.is_empty() {
                    if let Some(tx_id) = active_txn_id.take() {
                        if let Some(rust_store) = script_executor
                            .store()
                            .as_any()
                            .downcast_ref::<crate::storage::RustStore>()
                        {
                            if let Err(error) = rust_store.commit_local_transaction(tx_id) {
                                let _ = rust_store.rollback_local_transaction(tx_id);
                                tx_errors.push(format!("Commit failed: {}", error));
                            }
                        }
                    }
                }

                if tx_errors.is_empty() {
                    query_bindings = tx_bindings;
                    value_bindings = tx_value_bindings;
                    success_count += tx_success + 1;
                    results.extend(tx_results);
                    results.push(StatementResult::Transaction("committed".to_string()));
                    println!(
                        "🔒 Transaction committed ({} statements)",
                        tx_statements.len()
                    );
                } else {
                    if let Some(tx_id) = active_txn_id {
                        if let Some(rust_store) = script_executor
                            .store()
                            .as_any()
                            .downcast_ref::<crate::storage::RustStore>()
                        {
                            let _ = rust_store.rollback_local_transaction(tx_id);
                        }
                    }
                    if let Some(snapshot) = &snapshot {
                        restore_store(script_executor.store().as_ref(), snapshot)?;
                    }
                    // Definitions made inside the block go with it.
                    for name in &tx_functions {
                        script_executor.unregister_user_function(name);
                    }
                    let tx_msg = format!(
                        "Transaction rolled back due to {} error(s)",
                        tx_errors.len()
                    );
                    println!("❌ {}", tx_msg);
                    errors.push(tx_msg.clone());
                    errors.extend(tx_errors.into_iter());
                    results.push(StatementResult::Transaction("rolled_back".to_string()));
                    results.push(StatementResult::Error(tx_msg));
                }
            }
        }
    }

    if not_run > 0 {
        let message = format!(
            "{not_run} later statement(s) were not run because an earlier statement failed"
        );
        println!("⏭️  {}", message);
        errors.push(message);
    }

    println!(
        "\n📊 Summary: {}/{} statements executed successfully",
        success_count,
        schema_count + query_count + function_count + binding_count + transaction_count
    );
    println!("   - {} schemas", schema_count);
    println!("   - {} bindings", binding_count);
    println!("   - {} queries", query_count);
    println!("   - {} functions", function_count);
    println!("   - {} transactions", transaction_count);

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
    Binding(String),
    Query(usize), // Number of documents returned
    Function(String),
    Transaction(String),
    Error(String),
}

#[derive(Clone)]
struct StoreSnapshot {
    schemas: Vec<crate::schema::Schema>,
    documents_by_schema: std::collections::HashMap<String, Vec<crate::document::Document>>,
}

fn snapshot_store<S: crate::storage::DocumentStore>(
    store: &S,
) -> Result<StoreSnapshot, Box<dyn std::error::Error>> {
    let schema_names = store.list_schemas()?;
    let mut schemas = Vec::new();
    let mut documents_by_schema = std::collections::HashMap::new();
    for name in schema_names {
        if let Some(schema) = store.get_schema(&name)? {
            schemas.push(schema);
            let docs = store.scan_documents(&name)?;
            documents_by_schema.insert(name, docs);
        }
    }
    Ok(StoreSnapshot {
        schemas,
        documents_by_schema,
    })
}

fn restore_store<S: crate::storage::DocumentStore>(
    store: &S,
    snapshot: &StoreSnapshot,
) -> Result<(), Box<dyn std::error::Error>> {
    let existing_schemas = store.list_schemas()?;
    for schema_name in &existing_schemas {
        let docs = store.scan_documents(schema_name)?;
        for doc in docs {
            let _ = store.delete_document(schema_name, &doc.id)?;
        }
    }
    for schema_name in existing_schemas {
        let _ = store.delete_schema(&schema_name)?;
    }

    for schema in &snapshot.schemas {
        store.put_schema(schema)?;
    }
    for docs in snapshot.documents_by_schema.values() {
        for doc in docs {
            store.put_document(doc)?;
        }
    }
    Ok(())
}

/// Convert a SchemaDefinition to a Schema with resolved types
pub fn schema_definition_to_schema(
    def: &SchemaDefinition,
    registry: &crate::schema::SchemaRegistry,
) -> crate::error::OQLResult<crate::schema::Schema> {
    use crate::schema::{CascadeBehavior, FieldType, SchemaConstraint};
    use std::collections::HashMap;

    let invalid = |message: String| crate::error::OQLError::SchemaValidation { message };
    let mut fields = HashMap::new();
    let mut constraints = Vec::new();

    for field in def.fields.iter().filter(|field| !field.removed) {
        let mut shapes = Vec::new();
        let mut field_type =
            declared_field_type(&field.field_type, &field.name, registry, &mut shapes)?;
        constraints.extend(
            shapes
                .into_iter()
                .map(|(path, fields)| SchemaConstraint::Shape { path, fields }),
        );

        if let Some(rule) = &field.on_delete {
            fn reference(field_type: &mut FieldType) -> Option<&mut crate::schema::ReferenceType> {
                match field_type {
                    FieldType::Reference(reference) => Some(reference),
                    FieldType::Optional(inner) | FieldType::List(inner) => reference(inner),
                    _ => None,
                }
            }
            let reference = reference(&mut field_type).ok_or_else(|| {
                invalid(format!(
                    "'{}.{}': 'on delete' applies only to a reference",
                    def.name, field.name
                ))
            })?;
            reference.on_delete = match rule.as_str() {
                "cascade" => CascadeBehavior::Cascade,
                "nullify" => CascadeBehavior::SetNull,
                _ => CascadeBehavior::Restrict,
            };
        }
        if field.reverse_name.is_some() && field_type.referenced_schema().is_none() {
            return Err(invalid(format!(
                "'{}.{}': 'as' names the reverse of a reference",
                def.name, field.name
            )));
        }
        if field.on_delete.as_deref() == Some("nullify")
            && !field.optional
            && !matches!(field_type, FieldType::Optional(_) | FieldType::List(_))
        {
            return Err(invalid(format!(
                "'{}.{}': 'on delete nullify' needs an optional reference",
                def.name, field.name
            )));
        }

        let final_type = if field.optional {
            FieldType::Optional(Box::new(field_type))
        } else {
            field_type
        };
        fields.insert(field.name.clone(), final_type);

        if field.unique {
            constraints.push(SchemaConstraint::Unique {
                fields: vec![field.name.clone()],
                name: Some(format!("{}_{}_unique", def.name, field.name)),
            });
        }
        if let Some(expression) = &field.default {
            constraints.push(SchemaConstraint::Default {
                field: field.name.clone(),
                expression: expression.clone(),
            });
        }
        if let Some(name) = &field.reverse_name {
            constraints.push(SchemaConstraint::ReverseName {
                field: field.name.clone(),
                name: name.clone(),
            });
        }
    }

    for declaration in &def.declarations {
        let known = |name: &String| {
            fields
                .contains_key(name.split('.').next().unwrap_or(name))
                .then_some(())
                .ok_or_else(|| invalid(format!("'{}' has no field '{}'", def.name, name)))
        };
        match declaration {
            SchemaDeclaration::Unique(names) => {
                names.iter().try_for_each(known)?;
                constraints.push(SchemaConstraint::Unique {
                    fields: names.clone(),
                    name: Some(format!("{}_{}_unique", def.name, names.join("_"))),
                });
            }
            SchemaDeclaration::Check(expression) => {
                constraints.push(SchemaConstraint::CheckExpression {
                    expression: expression.clone(),
                });
            }
            SchemaDeclaration::Index(names) => names.iter().try_for_each(known)?,
            SchemaDeclaration::VectorIndex { field, .. } => known(field)?,
        }
    }

    Ok(crate::schema::Schema::new_with_constraints(
        def.name.clone(),
        fields,
        constraints,
    ))
}

/// The indexes a schema block declares, including the unique index behind
/// each `unique` field or `unique(..)` declaration.
pub fn schema_definition_indexes(
    def: &SchemaDefinition,
    schema: &crate::schema::Schema,
) -> crate::error::OQLResult<(
    Vec<crate::storage::ScalarIndexDefinition>,
    Vec<crate::storage::VectorIndexDefinition>,
)> {
    use crate::storage::{
        ScalarIndexDefinition, VectorIndexDefinition, VectorIndexState, VectorIndexType,
        VectorMetricType,
    };
    let scalar = |fields: &[String], unique: bool| ScalarIndexDefinition {
        name: format!(
            "{}_{}{}",
            def.name,
            if unique { "unique_" } else { "by_" },
            fields.join("_").replace('.', "_")
        ),
        schema: def.name.clone(),
        fields: fields.to_vec(),
        unique,
    };
    let mut scalars: Vec<ScalarIndexDefinition> = def
        .fields
        .iter()
        .filter(|field| field.unique && !field.removed)
        .map(|field| scalar(std::slice::from_ref(&field.name), true))
        .collect();
    let mut vectors = Vec::new();
    for declaration in &def.declarations {
        match declaration {
            SchemaDeclaration::Unique(names) => scalars.push(scalar(names, true)),
            SchemaDeclaration::Index(names) => scalars.push(scalar(names, false)),
            SchemaDeclaration::Check(_) => {}
            SchemaDeclaration::VectorIndex { field, metric } => {
                let dimension = match schema.fields.get(field) {
                    Some(crate::schema::FieldType::Vector { dim }) => *dim,
                    _ => {
                        return Err(crate::error::OQLError::SchemaValidation {
                            message: format!(
                                "'{}.{}' is not a Vector field, so it cannot have a vector index",
                                def.name, field
                            ),
                        })
                    }
                };
                vectors.push(VectorIndexDefinition {
                    name: format!("{}_by_{}", def.name, field),
                    schema: def.name.clone(),
                    field: field.clone(),
                    dimension,
                    metric: match metric.to_ascii_lowercase().as_str() {
                        "cosine" => VectorMetricType::Cosine,
                        "l2" => VectorMetricType::L2,
                        "dot" => VectorMetricType::Dot,
                        other => {
                            return Err(crate::error::OQLError::SchemaValidation {
                                message: format!(
                                    "'{other}' is not a vector metric; use cosine, l2 or dot"
                                ),
                            })
                        }
                    },
                    index_type: VectorIndexType::Exact,
                    quantization: None,
                    state: VectorIndexState::Ready,
                });
            }
        }
    }
    Ok((scalars, vectors))
}

/// Resolve a declared type, collecting the shape of every embedded object
/// it describes. `address: { city: String }` is an `Object` field whose
/// shape is recorded under the path `address`; a list of such objects is
/// recorded under `items[]`.
fn declared_field_type(
    declared: &str,
    path: &str,
    registry: &crate::schema::SchemaRegistry,
    shapes: &mut Vec<(String, Vec<(String, crate::schema::FieldType)>)>,
) -> crate::error::OQLResult<crate::schema::FieldType> {
    use crate::schema::FieldType;
    let declared = declared.trim();
    let wrapped = |prefix: &str| {
        declared
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix(')'))
    };
    if let Some(inner) = declared.strip_suffix('?') {
        return Ok(FieldType::Optional(Box::new(declared_field_type(
            inner, path, registry, shapes,
        )?)));
    }
    if let Some(inner) = wrapped("Optional(") {
        return Ok(FieldType::Optional(Box::new(declared_field_type(
            inner, path, registry, shapes,
        )?)));
    }
    if let Some(inner) = wrapped("List(").or_else(|| wrapped("Array(")) {
        return Ok(FieldType::List(Box::new(declared_field_type(
            inner,
            &format!("{path}[]"),
            registry,
            shapes,
        )?)));
    }
    let Some(body) = declared
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
    else {
        return parse_field_type(declared, registry);
    };

    let base = path.trim_end_matches("[]");
    let mut fields = Vec::new();
    for entry in split_top_level(body, &[',', '\n']) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (name, nested) = entry.split_once(':').ok_or_else(|| {
            crate::error::OQLError::SchemaValidation {
                message: format!("in '{path}': expected 'name: Type', found '{entry}'"),
            }
        })?;
        let name = name.trim();
        fields.push((
            name.to_string(),
            declared_field_type(nested, &format!("{base}.{name}"), registry, shapes)?,
        ));
    }
    shapes.push((path.to_string(), fields));
    Ok(FieldType::Object)
}

fn parse_field_type(
    type_str: &str,
    registry: &crate::schema::SchemaRegistry,
) -> crate::error::OQLResult<crate::schema::FieldType> {
    crate::type_syntax::parse_declared_field_type(type_str, |name| registry.exists(name))
}

#[cfg(test)]
mod file_parser_regression_tests {
    use super::*;
    use crate::storage::RustStore;
    use crate::QueryExecutor;
    use std::sync::Arc;
    use tempfile::TempDir;

    #[test]
    fn test_parse_select_with_aggregate_aliases_with_keyword_prefixes() {
        let query = r#"orders.group_by { .user_id }.select { user_id, total_orders: count(), total_amount: sum(.amount), avg_amount: avg(.amount) }"#;
        let parsed = parse_oql(query);
        assert!(
            parsed.is_ok(),
            "expected query to parse, got: {:?}",
            parsed.err()
        );
    }

    #[test]
    fn test_parse_oql_file_consecutive_statements_without_blank_lines() {
        let script = r#"
schema orders {
  id: String,
  user_id: String,
  amount: Float,
  status: String
}
orders.insert { id: "o1", user_id: "u1", amount: 320.0, status: "paid" }
orders.insert { id: "o2", user_id: "u1", amount: 120.0, status: "pending" }
orders.group_by { .user_id }.select { user_id, total_orders: count(), total_amount: sum(.amount), avg_amount: avg(.amount) }
"#;
        let parsed = parse_oql_file(script);
        assert!(
            parsed.is_ok(),
            "expected script to parse, got: {:?}",
            parsed.err()
        );
        let file = parsed.unwrap();
        assert_eq!(file.statements.len(), 4);
    }

    #[test]
    fn test_parse_oql_file_query_binding_statement() {
        let script = r#"
openOrders = orders.where { .status == "paid" }
openOrders.select { id, amount }
"#;
        let parsed = parse_oql_file(script);
        assert!(
            parsed.is_ok(),
            "expected script to parse, got: {:?}",
            parsed.err()
        );
        let file = parsed.unwrap();
        assert_eq!(file.statements.len(), 2);
        match &file.statements[0] {
            OQLFileStatement::QueryBinding(binding) => {
                assert_eq!(binding.name, "openOrders");
                assert_eq!(binding.query.collection, "orders");
            }
            other => panic!("expected query binding, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_oql_file_transaction_block() {
        let script = r#"
schema users {
  id: String
}

transaction {
  users.insert { id: "u1" }
  users.insert { id: "u2" }
}
"#;
        let parsed = parse_oql_file(script).expect("parse script");
        assert_eq!(parsed.statements.len(), 2);
        match &parsed.statements[1] {
            OQLFileStatement::Transaction(inner) => {
                assert_eq!(inner.len(), 2);
                assert!(matches!(inner[0], OQLFileStatement::Query(_)));
                assert!(matches!(inner[1], OQLFileStatement::Query(_)));
            }
            other => panic!("expected transaction statement, got {:?}", other),
        }
    }

    #[test]
    fn test_resolve_query_with_bindings_composes_operations() {
        let base = OQLQuery {
            collection: "orders".to_string(),
            operations: vec![Operation::Where(WhereBlock {
                condition: Condition::Comparison {
                    left: Expression::FieldAccess(FieldAccess {
                        path: vec!["status".to_string()],
                    }),
                    operator: ComparisonOperator::Equal,
                    right: Expression::Literal(Literal::String("paid".to_string())),
                },
            })],
        };
        let mut bindings = std::collections::HashMap::new();
        bindings.insert("openOrders".to_string(), base);
        let query = parse_oql("openOrders.select { id }").expect("query parse");
        let resolved = resolve_query_with_bindings(&query, &bindings).expect("resolve");
        assert_eq!(resolved.collection, "orders");
        assert_eq!(resolved.operations.len(), 2);
        assert!(matches!(resolved.operations[0], Operation::Where(_)));
        assert!(matches!(resolved.operations[1], Operation::Select(_)));
    }

    #[test]
    fn test_resolve_query_with_bindings_inside_query_expression() {
        let mut bindings = std::collections::HashMap::new();
        bindings.insert(
            "openFindings".to_string(),
            parse_oql(r#"findings.where { .status == "open" }"#).expect("binding parse"),
        );
        let query = parse_oql(
            r#"users.select { id, c: openFindings.where { .severity == "high" }.count() }"#,
        )
        .expect("query parse");
        let resolved = resolve_query_with_bindings(&query, &bindings).expect("resolve");
        match &resolved.operations[0] {
            Operation::Select(block) => match &block.fields[1].expression {
                Expression::QueryExpression { query } => {
                    assert_eq!(query.collection, "findings");
                    assert_eq!(query.operations.len(), 3);
                    assert!(matches!(query.operations[0], Operation::Where(_)));
                    assert!(matches!(query.operations[1], Operation::Where(_)));
                    assert!(matches!(query.operations[2], Operation::Count));
                }
                other => panic!("expected query expression, got {:?}", other),
            },
            _ => panic!("Expected SELECT operation"),
        }
    }

    #[test]
    fn test_parse_improved_syntax_file() {
        let content = include_str!("../oql/improved_syntax.oql");
        let parsed = parse_oql_file(content);
        assert!(
            parsed.is_ok(),
            "expected improved_syntax.oql to parse, got: {:?}",
            parsed.err()
        );
    }

    #[test]
    fn test_parse_transactions_phase1_file() {
        let content = include_str!("../oql/transactions_phase1.oql");
        let parsed = parse_oql_file(content);
        assert!(
            parsed.is_ok(),
            "expected transactions_phase1.oql to parse, got: {:?}",
            parsed.err()
        );
    }

    #[test]
    fn test_execute_improved_syntax_file_smoke() {
        use crate::storage::RustStore;
        use crate::QueryExecutor;
        use std::sync::Arc;
        use tempfile::TempDir;

        let content = include_str!("../oql/improved_syntax.oql");
        let file = parse_oql_file(content).expect("parse improved_syntax");

        let temp_dir = TempDir::new().expect("temp dir");
        let path = temp_dir.path().join("improved_syntax_test.owl");
        let store = Arc::new(
            RustStore::new(path.to_str().expect("path"), 10 * 1024 * 1024).expect("store"),
        );
        let executor = QueryExecutor::new_with_real_clock(store);
        let result = execute_oql_file(&file, &executor).expect("execute improved_syntax");

        assert!(
            result.errors.is_empty(),
            "expected no execution errors: {:?}",
            result.errors
        );
        assert_eq!(result.schema_count, 4);
        assert!(result.function_count >= 4);
    }

    #[test]
    fn test_execute_functions_litmus_file_smoke() {
        use crate::storage::RustStore;
        use crate::QueryExecutor;
        use std::sync::Arc;
        use tempfile::TempDir;

        let content = include_str!("../oql/functions_litmus_test.oql");
        let file = parse_oql_file(content).expect("parse functions_litmus_test");

        let temp_dir = TempDir::new().expect("temp dir");
        let path = temp_dir.path().join("functions_litmus_test.owl");
        let store = Arc::new(
            RustStore::new(path.to_str().expect("path"), 10 * 1024 * 1024).expect("store"),
        );
        let executor = QueryExecutor::new_with_real_clock(store);
        let result = execute_oql_file(&file, &executor).expect("execute functions_litmus_test");

        assert!(
            result.errors.is_empty(),
            "expected no execution errors: {:?}",
            result.errors
        );
        assert_eq!(result.schema_count, 2);
        assert_eq!(result.function_count, 4);
        assert_eq!(result.query_count, 5);
    }

    #[test]
    fn test_execute_phase1_acceptance_file_smoke() {
        use crate::storage::RustStore;
        use crate::QueryExecutor;
        use std::sync::Arc;
        use tempfile::TempDir;

        let content = include_str!("../oql/phase1_acceptance.oql");
        let file = parse_oql_file(content).expect("parse phase1_acceptance");

        let temp_dir = TempDir::new().expect("temp dir");
        let path = temp_dir.path().join("phase1_acceptance_test.owl");
        let store = Arc::new(
            RustStore::new(path.to_str().expect("path"), 10 * 1024 * 1024).expect("store"),
        );
        let executor = QueryExecutor::new_with_real_clock(store);
        let result = execute_oql_file(&file, &executor).expect("execute phase1_acceptance");

        assert!(
            result.errors.is_empty(),
            "expected no execution errors: {:?}",
            result.errors
        );
        assert_eq!(result.schema_count, 2);
        assert_eq!(result.function_count, 2);
        assert_eq!(result.query_count, 5);
        assert_eq!(result.success_count, 10);
    }

    #[test]
    fn test_execute_transaction_rollback_restores_state() {
        let script = r#"
schema users {
  id: String
}

transaction {
  users.insert { id: "u1" }
  users.where { missing_function(*) }.select { id }
}
"#;
        let file = parse_oql_file(script).expect("parse script");
        let temp_dir = TempDir::new().expect("temp dir");
        let path = temp_dir.path().join("tx_rollback.owl");
        let store = Arc::new(
            RustStore::new(path.to_str().expect("path"), 10 * 1024 * 1024).expect("store"),
        );
        let executor = QueryExecutor::new_with_real_clock(store);
        let result = execute_oql_file(&file, &executor).expect("execute file");

        assert!(result
            .results
            .iter()
            .any(|r| matches!(r, StatementResult::Transaction(state) if state == "rolled_back")));

        let users = parse_oql("users.select { id }").expect("query parse");
        let docs = executor.execute_query(&users).expect("query execute");
        assert_eq!(
            docs.len(),
            0,
            "transaction rollback should restore pre-state"
        );
    }

    #[test]
    fn test_debug_improved_syntax_statement_boundaries() {
        let content = include_str!("../oql/improved_syntax.oql");
        let statements = split_statements(content);
        for (idx, statement) in statements.iter().enumerate() {
            let trimmed = statement.trim();
            if trimmed.starts_with("schema ") {
                parse_schema_definition(statement).unwrap_or_else(|e| {
                    panic!(
                        "schema parse failed at statement {}: {}\n{}",
                        idx, e, statement
                    )
                });
                continue;
            }
            if trimmed.starts_with("fn ") {
                parse_oql_statement(statement).unwrap_or_else(|e| {
                    panic!(
                        "function parse failed at statement {}: {}\n{}",
                        idx, e, statement
                    )
                });
                continue;
            }
            if parse_binding_statement(statement)
                .expect("binding parse attempt")
                .is_some()
            {
                continue;
            }
            parse_oql(statement).unwrap_or_else(|e| {
                panic!(
                    "query parse failed at statement {}: {}\n{}",
                    idx, e, statement
                )
            });
        }
    }

    #[test]
    fn test_schema_definition_unknown_type_fails() {
        let def = SchemaDefinition {
            name: "users".to_string(),
            fields: vec![SchemaField {
                name: "geo".to_string(),
                field_type: "GeoPoint".to_string(),
                ..SchemaField::default()
            }],
            declarations: Vec::new(),
        };
        let registry = crate::schema::SchemaRegistry::new();
        let res = schema_definition_to_schema(&def, &registry);
        assert!(res.is_err(), "expected unknown field type to error");
    }

    #[test]
    fn test_execute_oql_file_function_def_registers_successfully() {
        let temp_dir = TempDir::new().expect("temp dir");
        let path = temp_dir.path().join("test.owl");
        let store = Arc::new(
            RustStore::new(path.to_str().expect("path"), 10 * 1024 * 1024).expect("store"),
        );
        let executor = QueryExecutor::new_with_real_clock(store);
        let file = OQLFile {
            statements: vec![OQLFileStatement::FunctionDef(FunctionDefinition {
                name: "f".to_string(),
                parameters: vec![],
                return_type: Some("Integer".to_string()),
                body: Expression::Literal(Literal::Integer(1)),
            })],
        };

        let result = execute_oql_file(&file, &executor).expect("execute file");
        assert_eq!(result.function_count, 1);
        assert_eq!(result.success_count, 1);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_legacy_outer_anchor_is_rejected_at_parse_time() {
        let err = parse_oql(
            r#"companies.select { id, finding_count: findings.where { .company_id == outer.id }.count() }"#,
        )
        .expect_err("legacy outer.* should fail at parse time");
        let msg = err.to_string();
        assert!(
            msg.contains("Legacy scope anchor 'outer' is not supported"),
            "unexpected parse error: {msg}"
        );
    }

    #[test]
    fn test_parse_nearest_operation() {
        let query = parse_oql(
            r#"products.nearest { field: .embedding, query: [0.1, 0.2, 0.3], metric: "cosine", k: 3 }"#,
        )
        .expect("parse nearest");

        assert_eq!(query.collection, "products");
        assert_eq!(query.operations.len(), 1);
        match &query.operations[0] {
            Operation::Nearest(block) => {
                assert_eq!(block.field.path, vec!["embedding".to_string()]);
                assert_eq!(block.metric, VectorMetric::Cosine);
                assert_eq!(block.k, 3);
            }
            other => panic!("expected nearest operation, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_nearest_rejects_unknown_metric() {
        let err = parse_oql(
            r#"products.nearest { field: .embedding, query: [0.1, 0.2], metric: "manhattan", k: 2 }"#,
        )
        .expect_err("metric should fail");
        assert!(err.to_string().contains("Unsupported nearest metric"));
    }

    #[test]
    fn sort_direction_is_not_part_of_the_field_path() {
        for source in [
            "events.sort { .timestamp.desc }",
            "events.sort { .timestamp desc }",
        ] {
            let query = parse_oql(source).expect("parse descending sort");
            let Operation::Sort(block) = &query.operations[0] else {
                panic!("expected sort operation");
            };
            assert_eq!(block.fields[0].direction, SortDirection::Desc);
            let Expression::FieldAccess(field) = &block.fields[0].expression else {
                panic!("expected field access");
            };
            assert_eq!(field.path, ["timestamp"]);
        }
    }
}
