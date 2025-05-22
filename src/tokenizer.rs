#[derive(Debug, PartialEq)]
pub enum TokenKind {
    Ident,
    Number,
    String,
    Operator,
    LBrace,
    RBrace,
    Colon,
    Comma,
    Dot,
    LParen,
    RParen,
    Keyword,
    True,
    False,
    Null,
    EOF,
    LBracket,
    RBracket,
}

#[derive(Debug)]
pub struct Token {
    pub kind: TokenKind,
    pub value: String,
    pub pos: usize,
}

pub fn tokenize(input: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    let mut pos = 0;

    while let Some(&ch) = chars.peek() {
        match ch {
            ' ' | '\n' | '\t' => {
                chars.next();
                pos += 1;
            }
            '{' => {
                tokens.push(Token {
                    kind: TokenKind::LBrace,
                    value: "{".to_string(),
                    pos,
                });
                chars.next();
                pos += 1;
            }
            '}' => {
                tokens.push(Token {
                    kind: TokenKind::RBrace,
                    value: "}".to_string(),
                    pos,
                });
                chars.next();
                pos += 1;
            }
            '[' => {
                tokens.push(Token {
                    kind: TokenKind::LBracket,
                    value: "[".to_string(),
                    pos,
                });
                chars.next();
                pos += 1;
            }
            ']' => {
                tokens.push(Token {
                    kind: TokenKind::RBracket,
                    value: "]".to_string(),
                    pos,
                });
                chars.next();
                pos += 1;
            }
            '(' => {
                tokens.push(Token {
                    kind: TokenKind::LParen,
                    value: "(".to_string(),
                    pos,
                });
                chars.next();
                pos += 1;
            }
            ')' => {
                tokens.push(Token {
                    kind: TokenKind::RParen,
                    value: ")".to_string(),
                    pos,
                });
                chars.next();
                pos += 1;
            }
            ',' => {
                tokens.push(Token {
                    kind: TokenKind::Comma,
                    value: ",".to_string(),
                    pos,
                });
                chars.next();
                pos += 1;
            }
            ':' => {
                let next = chars.clone().nth(1);
                if next == Some(':') {
                    tokens.push(Token {
                        kind: TokenKind::Operator,
                        value: "::".to_string(),
                        pos,
                    });
                    chars.next();
                    chars.next();
                    pos += 2;
                } else {
                    tokens.push(Token {
                        kind: TokenKind::Colon,
                        value: ":".to_string(),
                        pos,
                    });
                    chars.next();
                    pos += 1;
                }
            }
            '+' | '*' | '/' | '=' => {
                let op = ch.to_string();
                chars.next();
                let next = chars.peek();
                if let Some(&'=') = next {
                    tokens.push(Token {
                        kind: TokenKind::Operator,
                        value: format!("{}=", op),
                        pos,
                    });
                    chars.next();
                    pos += 2;
                } else {
                    tokens.push(Token {
                        kind: TokenKind::Operator,
                        value: op,
                        pos,
                    });
                    pos += 1;
                }
            }
            '-' => {
                chars.next();
                let next = chars.peek();
                if let Some(&'>') = next {
                    tokens.push(Token {
                        kind: TokenKind::Operator,
                        value: "->".to_string(),
                        pos,
                    });
                    chars.next();
                    pos += 2;
                } else {
                    tokens.push(Token {
                        kind: TokenKind::Operator,
                        value: "-".to_string(),
                        pos,
                    });
                    pos += 1;
                }
            }
            '>' | '<' => {
                let op = ch.to_string();
                chars.next();
                let next = chars.peek();
                if let Some(&'=') = next {
                    tokens.push(Token {
                        kind: TokenKind::Operator,
                        value: format!("{}=", op),
                        pos,
                    });
                    chars.next();
                    pos += 2;
                } else if op == ">" && next == Some(&'=') {
                    tokens.push(Token {
                        kind: TokenKind::Operator,
                        value: ">=".to_string(),
                        pos,
                    });
                    chars.next();
                    pos += 2;
                } else {
                    tokens.push(Token {
                        kind: TokenKind::Operator,
                        value: op,
                        pos,
                    });
                    pos += 1;
                }
            }
            '!' => {
                chars.next();
                let next = chars.peek();
                if let Some(&'=') = next {
                    tokens.push(Token {
                        kind: TokenKind::Operator,
                        value: "!=".to_string(),
                        pos,
                    });
                    chars.next();
                    pos += 2;
                } else {
                    tokens.push(Token {
                        kind: TokenKind::Operator,
                        value: "!".to_string(),
                        pos,
                    });
                    pos += 1;
                }
            }
            '"' => {
                chars.next();
                pos += 1;
                let mut value = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch == '"' {
                        chars.next();
                        pos += 1;
                        break;
                    }
                    value.push(ch);
                    chars.next();
                    pos += 1;
                }
                let len = value.len();
                tokens.push(Token {
                    kind: TokenKind::String,
                    value,
                    pos: pos - len - 2,
                });
            }
            '.' => {
                tokens.push(Token {
                    kind: TokenKind::Dot,
                    value: ".".to_string(),
                    pos,
                });
                chars.next();
                pos += 1;
            }
            ch if ch.is_alphabetic() || ch == '_' => {
                let mut value = String::new();
                let start_pos = pos;
                while let Some(&ch) = chars.peek() {
                    if ch.is_alphanumeric() || ch == '_' {
                        value.push(ch);
                        chars.next();
                        pos += 1;
                    } else {
                        break;
                    }
                }
                let kind = match value.as_str() {
                    "fn" | "type" | "enum" | "by" | "as" | "asc" | "desc" => TokenKind::Keyword,
                    "true" => TokenKind::True,
                    "false" => TokenKind::False,
                    "null" => TokenKind::Null,
                    _ => TokenKind::Ident,
                };
                tokens.push(Token {
                    kind,
                    value,
                    pos: start_pos,
                });
            }
            ch if ch.is_digit(10) => {
                let mut value = String::new();
                let start_pos = pos;
                while let Some(&ch) = chars.peek() {
                    if ch.is_digit(10) || ch == '.' {
                        value.push(ch);
                        chars.next();
                        pos += 1;
                    } else {
                        break;
                    }
                }
                tokens.push(Token {
                    kind: TokenKind::Number,
                    value,
                    pos: start_pos,
                });
            }
            _ => {
                chars.next();
                pos += 1; // Skip unrecognized characters
            }
        }
    }
    tokens.push(Token {
        kind: TokenKind::EOF,
        value: "".to_string(),
        pos,
    });
    tokens
}
